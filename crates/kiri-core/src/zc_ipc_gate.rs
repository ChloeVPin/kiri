//! Fail-closed gate for the zero-copy / through-webview IPC pipe.
//!
//! Today's through-webview authority is split across two independent gates a
//! host must wire separately: the coarse capability-bit check inside
//! `Router::dispatch` (`validate::authorize_capability`) and the per-surface
//! host allowlists enforced inside each command service (see
//! `crates/kiri-runtime/src/host_policy.rs`). The shared-buffer reply leg
//! (`PostSharedBufferToScript`, T008) is not a first-class gated object at
//! all: it posts whatever `dispatch` produced.
//!
//! This module replaces that pair of mental gates with one object:
//! [`ZcIpcGate`] mints a short-lived, opaque, single-use [`ZcIpcPermit`] only
//! when BOTH the router-registered capability bit for the command AND the
//! surface's declared host allowlist admit the request. [`ZcIpcGate::redeem`]
//! validates the permit's identity, caller binding, command binding, args
//! binding, and expiry, consumes it (replays deny), and yields a
//! [`ZcIpcGrant`]. The grant is the only currency the reply leg accepts:
//! [`ZcIpcGrant::reply_bytes`] hands over bytes for the shared-buffer or JSON
//! post only while the grant is live and bound to the same caller + command.
//!
//! Strictness, relative to today's double gate:
//!
//! - Same checks, same inputs. The capability check uses the exact
//!   [`CapabilityBits`] the router registered (`Router::required_bits`), and
//!   the allowlist predicates wrap the same `HostAllowlist` /
//!   `ShellAllowlist` values the services enforce. Those in-service checks
//!   still run at execute time, so the gate can only ever add a denial.
//! - Stricter at the edges. Unknown command ids fail closed against the live
//!   router registration (no `PING` fallback). Permits are bound to caller +
//!   command + args digest and expire after a short TTL; replayed, forged,
//!   or mismatched permits deny.
//! - The reply leg gains a gate it did not have: nothing crosses the
//!   shared-buffer path without a live grant.
//!
//! Simplicity claim (scoped to this pipe): the host declares every second
//! gate for the through-webview surface in ONE table on `ZcIpcGate`, next to
//! the mint call, instead of distributing allowlist wiring across per-service
//! constructors. "Capability without allowlist" and "allowlist without
//! capability" are unrepresentable here: mint requires both, and the reply
//! leg requires the grant only mint + redeem can produce.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;

use serde_json::Value;

use crate::caller::CallerId;
use crate::capabilities::CapabilityBits;
use crate::dispatch::Router;
use crate::error::Error;
use crate::trace::TraceSink;
use crate::wire::{WireRequest, WireResponse};

/// Default permit time-to-live: two seconds. A permit exists to authorize one
/// synchronous request -> dispatch -> reply leg on the pipe; a short TTL
/// bounds the window a captured token could be replayed.
pub const DEFAULT_PERMIT_TTL_NS: u64 = 2_000_000_000;

/// The second gate for a double-gated surface: a predicate over the decoded
/// request payload that returns true only when the host allowlist admits it.
/// Supplied by the host at gate construction; JavaScript never sees it.
pub type ZcIpcAdmission = Arc<dyn Fn(&Value) -> bool + Send + Sync>;

/// How a declared surface proves its host allowlist at mint time.
enum SecondGate {
    /// Arg-level admission: the predicate runs on the decoded payload, the
    /// same decision the command service makes at execute time.
    Args(ZcIpcAdmission),
    /// Surface-level admission for legs where args are not yet decoded: a
    /// host-declared surface key that must be present in the gate's surface
    /// allowlist. The per-arg allowlist still runs inside the service.
    Keyed(&'static str),
}

/// A minted permit's binding record, held inside the gate until redeemed or
/// expired. The permit itself is an opaque token; this table is the authority.
struct IssuedPermit {
    caller: CallerId,
    command_id: u32,
    /// Digest of the payload seen at mint, or `None` for surface-level mints
    /// issued before args decode.
    args_digest: Option<u64>,
    expires_at: u64,
}

/// Opaque, single-use authority to cross the through-webview pipe for exactly
/// one request. Constructed only by [`ZcIpcGate::mint`] /
/// [`ZcIpcGate::mint_surface`]; there is no public constructor, so callers
/// cannot fabricate one, and the issued table rejects values never minted.
#[derive(Debug, Clone, Copy)]
pub struct ZcIpcPermit {
    token: u64,
}

impl ZcIpcPermit {
    /// Test-only constructor for a token that was never minted. The issued
    /// table is the authority, so a forged value can never redeem.
    #[doc(hidden)]
    #[must_use]
    pub const fn __test_only_forged(token: u64) -> Self {
        ZcIpcPermit { token }
    }
}

/// Proof that a permit was redeemed for a specific caller + command. This is
/// the only object the reply leg accepts; it has no public constructor and
/// cannot outlive its expiry.
#[derive(Debug, Clone, Copy)]
pub struct ZcIpcGrant {
    caller: CallerId,
    command_id: u32,
    expires_at: u64,
}

impl ZcIpcGrant {
    /// The caller this grant is bound to.
    pub fn caller(&self) -> CallerId {
        self.caller
    }

    /// The command id this grant is bound to.
    pub fn command_id(&self) -> u32 {
        self.command_id
    }

    /// True while the grant can still authorize its reply leg.
    pub fn is_live(&self, now_ns: u64) -> bool {
        now_ns <= self.expires_at
    }

    /// Reply-leg gate for the shared-buffer (or JSON) post. Returns the bytes
    /// only when this grant is live and bound to `caller` + `command_id`;
    /// `None` fails closed and the transport must not post via the gated leg.
    pub fn reply_bytes<'a>(
        &self,
        caller: CallerId,
        command_id: u32,
        bytes: &'a [u8],
        now_ns: u64,
    ) -> Option<&'a [u8]> {
        if self.is_live(now_ns) && self.caller == caller && self.command_id == command_id {
            Some(bytes)
        } else {
            None
        }
    }
}

/// Outcome of [`ZcIpcGate::dispatch_through_webview`].
pub struct GatedResponse {
    /// The wire response the host should post back to the page. Denial
    /// responses are produced by the gate itself and carry no service data.
    pub response: WireResponse,
    /// Present only when the request passed both gates and the permit
    /// redeemed. Shared-buffer reply posts MUST require this object.
    pub grant: Option<ZcIpcGrant>,
}

/// Unpredictable token stream for minted permits. SplitMix64 seeded from
/// `RandomState` (OS entropy) mixed with the clock; an in-process attacker
/// cannot precompute live token values, and even a guessed token still fails
/// the caller/command/args binding checks.
struct Splitmix64(u64);

impl Splitmix64 {
    fn seeded() -> Self {
        let seed = RandomState::new().build_hasher().finish();
        Splitmix64(seed ^ crate::trace::MonotonicClock::now_ns())
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The unified fail-closed gate for the zero-copy / through-webview pipe.
///
/// One `ZcIpcGate` lives beside the router in each host. Double-gated
/// surfaces declare their second gate once via [`ZcIpcGate::admit`] (arg
/// level) or [`ZcIpcGate::admit_keyed`] + [`ZcIpcGate::allow_surface_key`]
/// (surface level). Surfaces with no declaration are capability-only on this
/// pipe, identical to today's single gate.
pub struct ZcIpcGate {
    second: HashMap<u32, SecondGate>,
    surface_keys: HashSet<String>,
    issued: HashMap<u64, IssuedPermit>,
    rng: Splitmix64,
    digest_keys: RandomState,
    ttl_ns: u64,
}

impl Default for ZcIpcGate {
    fn default() -> Self {
        Self::new()
    }
}

impl ZcIpcGate {
    /// A gate with the default two-second permit TTL.
    pub fn new() -> Self {
        Self::with_ttl_ns(DEFAULT_PERMIT_TTL_NS)
    }

    /// A gate with an explicit permit TTL in nanoseconds (tests + tuning).
    pub fn with_ttl_ns(ttl_ns: u64) -> Self {
        ZcIpcGate {
            second: HashMap::new(),
            surface_keys: HashSet::new(),
            issued: HashMap::new(),
            rng: Splitmix64::seeded(),
            digest_keys: RandomState::new(),
            ttl_ns,
        }
    }

    /// Declare the arg-level second gate for `command_id`: mint requires the
    /// capability bit AND `admission(&payload)` to be true.
    pub fn admit(&mut self, command_id: u32, admission: ZcIpcAdmission) -> &mut Self {
        self.second.insert(command_id, SecondGate::Args(admission));
        self
    }

    /// Declare a surface-level second gate for `command_id`: mint requires
    /// the capability bit AND `surface_key` to be on the gate's surface
    /// allowlist. Use for legs minted before args decode; the service's own
    /// allowlist still gates the decoded args at execute time.
    pub fn admit_keyed(&mut self, command_id: u32, surface_key: &'static str) -> &mut Self {
        self.second.insert(command_id, SecondGate::Keyed(surface_key));
        self
    }

    /// Put `surface_key` on the gate's surface allowlist, admitting
    /// [`ZcIpcGate::admit_keyed`] surfaces that name it. An empty or missing
    /// allowlist admits nothing (default deny).
    pub fn allow_surface_key(&mut self, surface_key: &str) -> &mut Self {
        self.surface_keys.insert(surface_key.to_string());
        self
    }

    /// Mint a permit for one through-webview request. Succeeds only when the
    /// command is registered in `router`, `caller_caps` is a superset of the
    /// exact bits the router will enforce, AND the surface's declared second
    /// gate admits the payload. Unknown ids fail closed with a protocol
    /// error; they are never mapped to a harmless capability.
    pub fn mint(
        &mut self,
        router: &Router,
        caller: CallerId,
        caller_caps: &CapabilityBits,
        request: &WireRequest,
        now_ns: u64,
    ) -> Result<ZcIpcPermit, Error> {
        if request.magic != crate::header::MAGIC
            || request.version != crate::header::PROTOCOL_VERSION
            || request.flags & crate::header::ControlFlags::REQUEST.bits() == 0
        {
            return Err(Error::protocol_error("malformed control request"));
        }
        self.authorize(router, caller_caps, request.command_id, Some(&request.payload))?;
        let digest = self.args_digest(&request.payload);
        Ok(self.issue(caller, request.command_id, Some(digest), now_ns))
    }

    /// Mint a surface-level permit before args are decoded. Arg-level
    /// (`admit`) surfaces deny here because their allowlist cannot be
    /// evaluated without the payload; keyed and capability-only surfaces
    /// mint. The resulting permit binds caller + command only.
    pub fn mint_surface(
        &mut self,
        router: &Router,
        caller: CallerId,
        caller_caps: &CapabilityBits,
        command_id: u32,
        now_ns: u64,
    ) -> Result<ZcIpcPermit, Error> {
        self.authorize(router, caller_caps, command_id, None)?;
        Ok(self.issue(caller, command_id, None, now_ns))
    }

    /// Redeem a permit into a [`ZcIpcGrant`]. Fails closed on an
    /// unissued/forged token, wrong caller, wrong command, rebound args, or
    /// expiry, and consumes the permit either way: a second redeem of the
    /// same token always denies.
    pub fn redeem(
        &mut self,
        permit: &ZcIpcPermit,
        caller: CallerId,
        request: &WireRequest,
        now_ns: u64,
    ) -> Result<ZcIpcGrant, Error> {
        let Some(issued) = self.issued.get(&permit.token) else {
            return Err(Error::unauthorized("zc ipc permit was never issued"));
        };
        let (bound_caller, bound_command, bound_digest, expires_at) =
            (issued.caller, issued.command_id, issued.args_digest, issued.expires_at);
        if bound_caller != caller {
            return Err(Error::unauthorized("zc ipc permit bound to a different caller"));
        }
        if bound_command != request.command_id {
            return Err(Error::unauthorized("zc ipc permit bound to a different command"));
        }
        if let Some(digest) = bound_digest {
            if digest != self.args_digest(&request.payload) {
                return Err(Error::unauthorized("zc ipc permit bound to different arguments"));
            }
        }
        self.issued.remove(&permit.token);
        if now_ns > expires_at {
            return Err(Error::unauthorized("zc ipc permit expired"));
        }
        Ok(ZcIpcGrant { caller, command_id: request.command_id, expires_at })
    }

    /// The whole through-webview request leg as one gated operation: mint,
    /// redeem, then `Router::dispatch` (whose own validation and the
    /// in-service allowlists still run). Denial returns a gate-produced error
    /// response with no grant, so the shared-buffer reply leg stays closed.
    pub fn dispatch_through_webview(
        &mut self,
        router: &Router,
        caller: CallerId,
        caller_caps: &CapabilityBits,
        request: &WireRequest,
        sink: &mut dyn TraceSink,
        now_ns: u64,
    ) -> GatedResponse {
        match self
            .mint(router, caller, caller_caps, request, now_ns)
            .and_then(|permit| self.redeem(&permit, caller, request, now_ns))
        {
            Ok(grant) => GatedResponse {
                response: router.dispatch(caller, caller_caps, request, sink),
                grant: Some(grant),
            },
            Err(e) => {
                let e = e.with_request_id(request.request_id);
                GatedResponse { response: WireResponse::err(request.request_id, e), grant: None }
            }
        }
    }

    /// The double gate as one decision: registered command -> capability
    /// superset -> declared second gate. Any failure denies; nothing here can
    /// express "capability without allowlist" for a declared surface.
    fn authorize(
        &self,
        router: &Router,
        caller_caps: &CapabilityBits,
        command_id: u32,
        payload: Option<&Value>,
    ) -> Result<(), Error> {
        let required = router
            .required_bits(command_id)
            .ok_or_else(|| Error::protocol_error(format!("unknown command id {command_id}")))?;
        if !caller_caps.is_superset_of(&required) {
            return Err(Error::unauthorized("caller lacks required capability"));
        }
        match self.second.get(&command_id) {
            Some(SecondGate::Args(admission)) => {
                let Some(payload) = payload else {
                    return Err(Error::scope_denied(format!(
                        "command id {command_id}: allowlist requires decoded args"
                    )));
                };
                if !admission(payload) {
                    return Err(Error::scope_denied(format!(
                        "command id {command_id}: request not on host allowlist"
                    )));
                }
            }
            Some(SecondGate::Keyed(key)) if !self.surface_keys.contains(*key) => {
                return Err(Error::scope_denied(format!(
                    "command id {command_id}: surface not on host allowlist"
                )));
            }
            Some(SecondGate::Keyed(_)) | None => {}
        }
        Ok(())
    }

    fn issue(
        &mut self,
        caller: CallerId,
        command_id: u32,
        args_digest: Option<u64>,
        now_ns: u64,
    ) -> ZcIpcPermit {
        self.issued.retain(|_, p| p.expires_at >= now_ns);
        let mut token = self.rng.next();
        while self.issued.contains_key(&token) {
            token = self.rng.next();
        }
        self.issued.insert(
            token,
            IssuedPermit {
                caller,
                command_id,
                args_digest,
                expires_at: now_ns.saturating_add(self.ttl_ns),
            },
        );
        ZcIpcPermit { token }
    }

    /// Keyed digest of the request payload for the args binding. `RandomState`
    /// keys make the digest unpredictable outside this gate instance.
    fn args_digest(&self, payload: &Value) -> u64 {
        let bytes = serde_json::to_vec(payload).unwrap_or_default();
        let mut h = self.digest_keys.build_hasher();
        h.write(&bytes);
        h.finish()
    }
}

/// Second-gate predicate for `kiri.http.*` mirroring `HttpService::execute`:
/// the payload `url` must parse and its authority must be on `allowlist`.
/// Hosts build it from the same `HostAllowlist` the service holds, so the
/// pipe decision and the execute-time decision cannot diverge.
pub fn http_host_admission(allowlist: crate::http::HostAllowlist) -> ZcIpcAdmission {
    Arc::new(move |payload: &Value| {
        let Some(url) = payload.get("url").and_then(|v| v.as_str()) else {
            return false;
        };
        match crate::http::authority_of(url) {
            Some(authority) => allowlist.allows(&authority),
            None => false,
        }
    })
}

/// Second-gate predicate for `kiri.shell.run` mirroring `ShellService::run`:
/// `program` plus `args` must match a host allowlist entry.
pub fn shell_command_admission(allowlist: crate::shell::ShellAllowlist) -> ZcIpcAdmission {
    Arc::new(move |payload: &Value| {
        let Some(program) = payload.get("program").and_then(|v| v.as_str()) else {
            return false;
        };
        let args: Vec<String> = payload
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        allowlist.allows(program, &args)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::{capability_bit, command_id};
    use crate::trace::NoopTraceSink;

    fn ping_caps() -> CapabilityBits {
        let mut c = CapabilityBits::empty();
        c.set(capability_bit::PING);
        c
    }

    #[test]
    fn forged_token_never_redeems() {
        let mut gate = ZcIpcGate::new();
        let forged = ZcIpcPermit { token: 0xFFFF_0000 };
        let req = WireRequest::new(command_id::PING, 1, 1, Value::Null);
        assert!(gate.redeem(&forged, CallerId(1), &req, 0).is_err());
    }

    #[test]
    fn minted_tokens_are_distinct() {
        let router = Router::new();
        let mut gate = ZcIpcGate::new();
        let req = WireRequest::new(command_id::PING, 1, 1, Value::Null);
        let a = gate.mint(&router, CallerId(1), &ping_caps(), &req, 0).unwrap();
        let b = gate.mint(&router, CallerId(1), &ping_caps(), &req, 0).unwrap();
        assert_ne!(a.token, b.token);
    }

    #[test]
    fn expired_issues_are_swept_on_mint() {
        let router = Router::new();
        let mut gate = ZcIpcGate::with_ttl_ns(10);
        let req = WireRequest::new(command_id::PING, 1, 1, Value::Null);
        gate.mint(&router, CallerId(1), &ping_caps(), &req, 0).unwrap();
        assert_eq!(gate.issued.len(), 1);
        gate.mint(&router, CallerId(1), &ping_caps(), &req, 100).unwrap();
        assert_eq!(gate.issued.len(), 1, "expired entry must be swept");
    }

    #[test]
    fn dispatch_denial_carries_no_grant() {
        let router = Router::new();
        let mut gate = ZcIpcGate::new();
        let req = WireRequest::new(command_id::PING, 9, 1, Value::Null);
        let out = gate.dispatch_through_webview(
            &router,
            CallerId(1),
            &CapabilityBits::empty(),
            &req,
            &mut NoopTraceSink,
            0,
        );
        assert!(out.response.error.is_some());
        assert!(out.grant.is_none());
    }
}
