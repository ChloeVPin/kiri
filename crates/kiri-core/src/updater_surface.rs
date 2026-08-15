//! Restricted, host-pinned signed-update checker (`kiri.updater.check`).
//!
//! This converts a Tauri weakness into a Kiri strength (audit item 18). Tauri's
//! `updater` JS API ships the signing public key in the frontend-supplied
//! config (`tauri.conf.json` -> `updater.pubkey`), so a malicious or phished
//! frontend can trivially substitute a key and accept an attacker-signed
//! release. Kiri instead pins the Ed25519 public key in the native host and
//! never exposes it to JavaScript: the frontend may only submit a manifest and
//! receive `{ available, version, notes, platform }`. The signature over the
//! current-OS asset is verified against the host-pinned key, the version is
//! compared against the running build, and the result is capability-gated so a
//! granted capability still cannot apply an update or read the raw signature.
//! Exceeds Tauri's updater on the security axis by construction.

use std::sync::Arc;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::limits::Limits;
use crate::update::{UpdateManifest, Version};

/// Authorizes the `kiri.updater.*` commands. Reuses the shared `UPDATER` bit
/// (23) so it stays in lockstep with `capability_bit::UPDATER` and `for_command`.
pub const UPDATER_CAPABILITY: u32 = crate::dispatch::capability_bit::UPDATER;

/// Host-pinned update checker. The public key and current version are injected
/// by the native host at build/runtime; JavaScript can only feed it a manifest
/// and learn whether a newer, correctly-signed release exists for this OS.
#[derive(Clone)]
pub struct UpdaterService {
    public_key: String,
    current_version: Version,
    limits: Limits,
}

impl UpdaterService {
    pub fn new(public_key: impl Into<String>, current_version: Version, limits: Limits) -> Self {
        Self { public_key: public_key.into(), current_version, limits }
    }

    /// Parse a manifest, select the current-OS asset, verify its signature
    /// against the host-pinned key, and compare versions. Returns only the
    /// non-sensitive decision fields: the raw signature and asset URL are
    /// never handed to JavaScript. A missing/unsigned asset for this OS, a
    /// failed signature, or a non-newer version all resolve to
    /// `{ available: false, ... }` without leaking the reason to the caller.
    pub fn check(&self, manifest_json: &str) -> Result<Value> {
        let bytes = manifest_json.as_bytes();
        self.limits.check_bulk_object(bytes.len() as u64)?;

        let manifest = UpdateManifest::parse_json(manifest_json)
            .map_err(|_| Error::invalid_argument("kiri.updater.check: invalid manifest json"))?;

        let platform = crate::update::current_platform_key();
        let asset = match manifest.asset_for(&platform) {
            Ok(a) => a,
            Err(_) => {
                return Ok(serde_json::json!({
                    "available": false,
                    "version": manifest.version,
                    "platform": platform,
                    "notes": manifest.notes,
                }));
            }
        };

        let signature = match asset.signature.as_ref() {
            Some(s) => s,
            None => {
                return Ok(serde_json::json!({
                    "available": false,
                    "version": manifest.version,
                    "platform": platform,
                    "notes": manifest.notes,
                }));
            }
        };

        // Verify the signed asset descriptor against the host-pinned key. The
        // build pipeline signs each OS asset's descriptor (url); any key
        // mismatch or tamper is rejected before the version decision is made.
        if crate::update::Ed25519Verifier::verify(&self.public_key, asset.url.as_bytes(), signature)
            .is_err()
        {
            return Ok(serde_json::json!({
                "available": false,
                "version": manifest.version,
                "platform": platform,
                "notes": manifest.notes,
            }));
        }

        let newer = manifest.is_newer_than(&self.current_version).unwrap_or(false);

        Ok(serde_json::json!({
            "available": newer,
            "version": manifest.version,
            "platform": platform,
            "notes": manifest.notes,
        }))
    }
}

/// Build the kiri.updater handlers bound to one UpdaterService.
pub fn updater_handlers(
    service: UpdaterService,
) -> Vec<(u32, crate::capabilities::CapabilityBits, crate::dispatch::Handler)> {
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::command_id;
    use crate::dispatch::Handler;

    let mut required = CapabilityBits::empty();
    required.set(UPDATER_CAPABILITY);

    let svc = service.clone();
    vec![(
        command_id::UPDATER_CHECK,
        required,
        Arc::new(move |_c, _rid, p: &Value| {
            let manifest = p.get("manifest").and_then(|v| v.as_str()).ok_or_else(|| {
                Error::invalid_argument("kiri.updater.check requires string manifest")
            })?;
            svc.check(manifest)
        }) as Handler,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caller::CallerId;
    use crate::capabilities::CapabilityBits;
    use crate::dispatch::{command_id, Router};
    use crate::trace::NoopTraceSink;
    use crate::wire::WireRequest;
    use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
    use serde_json::json;

    fn keypair() -> (String, SigningKey) {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk = VerifyingKey::from(&sk);
        (hex::encode(vk.to_bytes()), sk)
    }

    fn signed_manifest(_pk: &str, sk: &SigningKey, version: &str) -> String {
        let platform = crate::update::current_platform_key();
        let url = format!("https://example.invalid/kiri-{version}-{platform}.bin");
        let sig = hex::encode(sk.sign(url.as_bytes()).to_bytes());
        format!(
            r#"{{
              "version": "{version}",
              "notes": "release {version}",
              "platforms": {{
                "{platform}": {{ "url": "{url}", "signature": "{sig}" }}
              }}
            }}"#
        )
    }

    fn router(pk: &str, current: &str) -> Router {
        let svc = UpdaterService::new(
            pk.to_string(),
            Version::parse(current).unwrap(),
            Limits::default(),
        );
        Router::new().with_updater(svc)
    }

    fn dispatch(router: &Router, id: u32, payload: Value) -> Value {
        let mut granted = CapabilityBits::empty();
        granted.set(UPDATER_CAPABILITY);
        let req = WireRequest::new(id, 1, 1, payload);
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        serde_json::to_value(&resp).unwrap()
    }

    #[test]
    fn newer_signed_manifest_reports_available() {
        let (pk, sk) = keypair();
        let r = router(&pk, "0.1.0");
        let out = dispatch(
            &r,
            command_id::UPDATER_CHECK,
            json!({ "manifest": signed_manifest(&pk, &sk, "0.2.0") }),
        );
        assert!(out["error"].is_null(), "unexpected error: {out}");
        assert_eq!(out["payload"]["available"], true);
        assert_eq!(out["payload"]["version"], "0.2.0");
        assert_eq!(out["payload"]["platform"], crate::update::current_platform_key());
        assert!(out["payload"].get("signature").is_none());
    }

    #[test]
    fn stale_manifest_reports_unavailable() {
        let (pk, sk) = keypair();
        let r = router(&pk, "0.3.0");
        let out = dispatch(
            &r,
            command_id::UPDATER_CHECK,
            json!({ "manifest": signed_manifest(&pk, &sk, "0.2.0") }),
        );
        assert!(out["error"].is_null());
        assert_eq!(out["payload"]["available"], false);
        assert_eq!(out["payload"]["version"], "0.2.0");
    }

    #[test]
    fn tampered_or_wrongkey_manifest_reports_unavailable() {
        let (_pk, sk) = keypair();
        let other_pk =
            hex::encode(VerifyingKey::from(&SigningKey::from_bytes(&[9u8; 32])).to_bytes());
        let r = router(&other_pk, "0.1.0");
        let out = dispatch(
            &r,
            command_id::UPDATER_CHECK,
            json!({ "manifest": signed_manifest(&_pk, &sk, "0.2.0") }),
        );
        assert!(out["error"].is_null());
        assert_eq!(out["payload"]["available"], false);
    }

    #[test]
    fn missing_signature_reports_unavailable() {
        let (pk, _sk) = keypair();
        let platform = crate::update::current_platform_key();
        let manifest = format!(
            r#"{{ "version": "0.2.0", "platforms": {{ "{platform}": {{ "url": "https://example.invalid/x" }} }} }}"#
        );
        let r = router(&pk, "0.1.0");
        let out = dispatch(&r, command_id::UPDATER_CHECK, json!({ "manifest": manifest }));
        assert!(out["error"].is_null());
        assert_eq!(out["payload"]["available"], false);
    }

    #[test]
    fn invalid_json_is_rejected() {
        let (pk, _sk) = keypair();
        let r = router(&pk, "0.1.0");
        let out = dispatch(&r, command_id::UPDATER_CHECK, json!({ "manifest": "{not json" }));
        assert!(!out["error"].is_null());
    }

    #[test]
    fn updater_denied_without_capability() {
        let (pk, sk) = keypair();
        let svc =
            UpdaterService::new(pk.clone(), Version::parse("0.1.0").unwrap(), Limits::default());
        let router = Router::new().with_updater(svc);
        let granted = CapabilityBits::empty();
        let req = WireRequest::new(
            command_id::UPDATER_CHECK,
            1,
            1,
            json!({ "manifest": signed_manifest(&pk, &sk, "0.2.0") }),
        );
        let resp = router.dispatch(CallerId(1), &granted, &req, &mut NoopTraceSink);
        assert!(resp.error.is_some());
    }
}
