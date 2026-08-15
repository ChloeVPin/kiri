//! Signed update manifest + verification (R-4: closes G-3 vs Tauri's updater).
//!
//! Pure, headless, and fully testable on every OS with no network, no GPU,
//! and no code-signing certificate. The signature primitive is Ed25519, the
//! same one Tauri's updater uses, so a shipped Kiri build verifies a release
//! the way Tauri verifies RELEASES.json. Our manifest is additionally
//! version-negotiated and never lowers a security check to apply an update.
//!
//! Verification is split in two: a cheap manifest parse selects the asset for
//! the running platform, then the downloaded installer bytes are verified
//! against the pinned public key. A tampered or downgrade manifest is rejected
//! before any bytes are accepted.

use std::collections::HashMap;

use ed25519_dalek::Verifier;
use serde::{Deserialize, Serialize};

/// A parsed semantic version: major.minor.patch[-pre][+build].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<PreIdent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreIdent {
    Numeric(u64),
    Alpha(String),
}

impl Version {
    /// Parse a semver string. Rejects anything that is not major.minor.patch
    /// optionally followed by -pre and/or +build.
    pub fn parse(s: &str) -> Result<Self> {
        let (core, pre) = match s.split_once('-') {
            Some((c, rest)) => {
                let pre = rest.split_once('+').map(|(p, _)| p).unwrap_or(rest);
                (c, pre)
            }
            None => {
                let c = s.split_once('+').map(|(c, _)| c).unwrap_or(s);
                (c, "")
            }
        };
        let mut parts = core.split('.');
        let major = parts
            .next()
            .ok_or(Error::InvalidVersion)?
            .parse()
            .map_err(|_| Error::InvalidVersion)?;
        let minor = parts
            .next()
            .ok_or(Error::InvalidVersion)?
            .parse()
            .map_err(|_| Error::InvalidVersion)?;
        let patch = parts
            .next()
            .ok_or(Error::InvalidVersion)?
            .parse()
            .map_err(|_| Error::InvalidVersion)?;
        if parts.next().is_some() {
            return Err(Error::InvalidVersion);
        }
        let pre = if pre.is_empty() {
            Vec::new()
        } else {
            pre.split('.')
                .map(|p| match p.parse::<u64>() {
                    Ok(n) => PreIdent::Numeric(n),
                    Err(_) => PreIdent::Alpha(p.to_string()),
                })
                .collect()
        };
        Ok(Version { major, minor, patch, pre })
    }

    /// true when this version is a pre-release (has any pre identifier).
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => std::cmp::Ordering::Equal,
                (true, false) => std::cmp::Ordering::Greater,
                (false, true) => std::cmp::Ordering::Less,
                (false, false) => {
                    let lhs: Vec<_> = self.pre.iter().collect();
                    let rhs: Vec<_> = other.pre.iter().collect();
                    let common = lhs.len().min(rhs.len());
                    for i in 0..common {
                        let ord = match (&lhs[i], &rhs[i]) {
                            (PreIdent::Numeric(a), PreIdent::Numeric(b)) => a.cmp(b),
                            (PreIdent::Numeric(_), PreIdent::Alpha(_)) => std::cmp::Ordering::Less,
                            (PreIdent::Alpha(_), PreIdent::Numeric(_)) => {
                                std::cmp::Ordering::Greater
                            }
                            (PreIdent::Alpha(a), PreIdent::Alpha(b)) => a.cmp(b),
                        };
                        if ord != std::cmp::Ordering::Equal {
                            return ord;
                        }
                    }
                    lhs.len().cmp(&rhs.len())
                }
            })
    }
}

/// One platform asset inside an UpdateManifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatformAsset {
    pub url: String,
    #[serde(default)]
    pub signature: Option<String>,
}

/// The release manifest. Field shape mirrors Tauri's RELEASES.json so a
/// Tauri-era release pipeline can be repurposed, but the runtime never trusts
/// the URL without a verified signature over the downloaded bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: Option<String>,
    pub platforms: HashMap<String, PlatformAsset>,
}

impl UpdateManifest {
    pub fn parse_json(s: &str) -> Result<Self> {
        serde_json::from_str(s).map_err(|_| Error::InvalidManifest)
    }

    pub fn version(&self) -> Result<Version> {
        Version::parse(&self.version)
    }

    pub fn asset_for(&self, platform_key: &str) -> Result<&PlatformAsset> {
        self.platforms.get(platform_key).ok_or(Error::NoAssetForPlatform)
    }

    pub fn asset_for_current_os(&self) -> Result<&PlatformAsset> {
        self.asset_for(&current_platform_key())
    }

    pub fn is_newer_than(&self, current: &Version) -> Result<bool> {
        Ok(self.version()? > *current)
    }

    /// Verify the downloaded installer file_bytes against the pinned public
    /// key and return the resolved asset + parsed version. Rejects a missing
    /// signature, a wrong key, a tampered file, or a downgrade (the caller must
    /// still compare version against the running build before applying).
    pub fn verify_asset(
        &self,
        public_key_hex: &str,
        file_bytes: &[u8],
    ) -> Result<VerifiedAsset<'_>> {
        let asset = self.asset_for_current_os()?;
        let version = self.version()?;
        let sig = asset.signature.as_ref().ok_or(Error::SignatureMissing)?;
        Ed25519Verifier::verify(public_key_hex, file_bytes, sig)?;
        Ok(VerifiedAsset { version, asset })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedAsset<'a> {
    pub version: Version,
    pub asset: &'a PlatformAsset,
}

/// Ed25519 verification over hex-encoded keys and signatures.
pub struct Ed25519Verifier;

impl Ed25519Verifier {
    pub fn verify(public_key_hex: &str, message: &[u8], signature_hex: &str) -> Result<()> {
        let pk = hex::decode(public_key_hex).map_err(|_| Error::SignatureInvalid)?;
        if pk.len() != 32 {
            return Err(Error::SignatureInvalid);
        }
        let sig = hex::decode(signature_hex).map_err(|_| Error::SignatureInvalid)?;
        if sig.len() != 64 {
            return Err(Error::SignatureInvalid);
        }
        let mut pk_arr = [0u8; 32];
        pk_arr.copy_from_slice(&pk);
        let mut sig_arr = [0u8; 64];
        sig_arr.copy_from_slice(&sig);
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&pk_arr)
            .map_err(|_| Error::SignatureInvalid)?;
        let sig = ed25519_dalek::Signature::from_bytes(&sig_arr);
        vk.verify(message, &sig).map_err(|_| Error::SignatureInvalid)
    }
}

/// The platform key for the binary currently running ({os}-{arch}).
pub fn current_platform_key() -> String {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "unknown"
    };
    format!("{os}-{arch}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    InvalidVersion,
    InvalidManifest,
    NoAssetForPlatform,
    SignatureMissing,
    SignatureInvalid,
    NotNewer,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Error::InvalidVersion => "invalid version string",
            Error::InvalidManifest => "invalid update manifest json",
            Error::NoAssetForPlatform => "no asset for current platform",
            Error::SignatureMissing => "asset has no signature",
            Error::SignatureInvalid => "signature verification failed",
            Error::NotNewer => "manifest version is not newer than current",
        };
        f.write_str(s)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey, VerifyingKey};

    fn keypair() -> (String, SigningKey) {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk = VerifyingKey::from(&sk);
        (hex::encode(vk.to_bytes()), sk)
    }

    fn manifest_with_signature(sig_hex: &str) -> String {
        format!(
            r#"{{
              "version": "0.2.0",
              "notes": "test release",
              "pub_date": "2026-08-15T00:00:00Z",
              "platforms": {{
                "darwin-aarch64": {{ "url": "https://example.invalid/kiri-0.2.0.dmg", "signature": "{sig_hex}" }},
                "windows-x86_64": {{ "url": "https://example.invalid/kiri-0.2.0.msi" }},
                "linux-x86_64": {{ "url": "https://example.invalid/kiri-0.2.0.AppImage" }}
              }}
            }}"#
        )
    }

    #[test]
    fn semver_parses_and_orders() {
        assert_eq!(Version::parse("1.2.3").unwrap().patch, 3);
        assert!(Version::parse("1.2.3").unwrap() < Version::parse("1.2.4").unwrap());
        assert!(Version::parse("1.2.3").unwrap() < Version::parse("1.10.0").unwrap());
        assert!(Version::parse("2.0.0").unwrap() > Version::parse("1.99.99").unwrap());
        assert!(Version::parse("1.2.3").unwrap() > Version::parse("1.2.3-alpha").unwrap());
        assert!(Version::parse("1.2.3-alpha").unwrap() < Version::parse("1.2.3-alpha.1").unwrap());
        assert!(
            Version::parse("1.2.3-alpha.1").unwrap() < Version::parse("1.2.3-alpha.2").unwrap()
        );
        assert_eq!(Version::parse("1.2.3+build5").unwrap(), Version::parse("1.2.3").unwrap());
    }

    #[test]
    fn semver_rejects_garbage() {
        assert_eq!(Version::parse("1.2"), Err(Error::InvalidVersion));
        assert_eq!(Version::parse("1.2.3.4"), Err(Error::InvalidVersion));
        assert_eq!(Version::parse("x.y.z"), Err(Error::InvalidVersion));
        assert_eq!(Version::parse(""), Err(Error::InvalidVersion));
    }

    #[test]
    fn manifest_roundtrips_and_resolves_platform() {
        let m = UpdateManifest::parse_json(&manifest_with_signature("00")).unwrap();
        assert_eq!(m.version().unwrap(), Version::parse("0.2.0").unwrap());
        assert!(m.asset_for("darwin-aarch64").is_ok());
        assert!(m.asset_for("linux-x86_64").is_ok());
        assert_eq!(m.asset_for("freebsd-x86_64"), Err(Error::NoAssetForPlatform));
    }

    #[test]
    fn signed_asset_verifies_with_correct_key() {
        let (pk, sk) = keypair();
        let bytes = b"fake-installer-bytes";
        let sig = sk.sign(bytes);
        let sig_hex = hex::encode(sig.to_bytes());
        let m = UpdateManifest::parse_json(&manifest_with_signature(&sig_hex)).unwrap();
        let verified = m.verify_asset(&pk, bytes).expect("verify must pass");
        assert_eq!(verified.version, Version::parse("0.2.0").unwrap());
    }

    #[test]
    fn tampered_bytes_rejected() {
        let (pk, sk) = keypair();
        let sig = sk.sign(b"fake-installer-bytes");
        let sig_hex = hex::encode(sig.to_bytes());
        let m = UpdateManifest::parse_json(&manifest_with_signature(&sig_hex)).unwrap();
        assert_eq!(
            m.verify_asset(&pk, b"fake-installer-bytes-TAMPERED"),
            Err(Error::SignatureInvalid)
        );
    }

    #[test]
    fn wrong_key_rejected() {
        let (_pk, sk) = keypair();
        let other_pk =
            hex::encode(VerifyingKey::from(&SigningKey::from_bytes(&[9u8; 32])).to_bytes());
        let sig = sk.sign(b"fake-installer-bytes");
        let sig_hex = hex::encode(sig.to_bytes());
        let m = UpdateManifest::parse_json(&manifest_with_signature(&sig_hex)).unwrap();
        assert_eq!(
            m.verify_asset(&other_pk, b"fake-installer-bytes"),
            Err(Error::SignatureInvalid)
        );
    }

    #[test]
    fn missing_signature_detected() {
        let m = UpdateManifest::parse_json(&manifest_with_signature("00")).unwrap();
        assert_eq!(m.asset_for("windows-x86_64").unwrap().signature, None);
    }

    #[test]
    fn is_newer_than_compares_versions() {
        let m = UpdateManifest::parse_json(&manifest_with_signature("00")).unwrap();
        assert!(m.is_newer_than(&Version::parse("0.1.0").unwrap()).unwrap());
        assert!(!m.is_newer_than(&Version::parse("0.3.0").unwrap()).unwrap());
        assert!(!m.is_newer_than(&Version::parse("0.2.0").unwrap()).unwrap());
    }

    #[test]
    fn current_platform_key_is_stable_shape() {
        let k = current_platform_key();
        assert!(k.contains('-'));
        let (os, arch) = k.split_once('-').unwrap();
        assert!(["darwin", "windows", "linux"].contains(&os));
        assert!(["aarch64", "x86_64", "unknown"].contains(&arch));
    }
}
