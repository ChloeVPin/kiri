//! Locking tests for the fail-closed `ZcIpcPermit` gate on the zero-copy /
//! through-webview IPC pipe (spike).
//!
//! Properties proven here, headlessly, with no WebView:
//!
//! 1. Missing capability DENIES even when the host allowlist would admit.
//! 2. Missing or empty host allowlist DENIES even when the capability is
//!    granted.
//! 3. Both present ALLOWS (mint succeeds) for a double-gated surface.
//! 4. Unknown command ids DENY (PR #31 semantics: no PING fallback).
//! 5. Forged, wrong-caller, wrong-command, rebound-args, replayed, and
//!    expired permits DENY at redeem.
//! 6. The through-webview / shared-buffer pipe helpers refuse to proceed
//!    without a valid permit/grant.

use std::sync::Arc;

use kiri_core::caller::CallerId;
use kiri_core::capabilities::CapabilityBits;
use kiri_core::dispatch::{capability_bit, command_id, Router};
use kiri_core::error::ErrorCode;
use kiri_core::http::{HostAllowlist, HttpClient, HttpRequest, HttpResponse, HttpService};
use kiri_core::limits::Limits;
use kiri_core::shell::{AllowedCommand, ShellAllowlist, ShellOutput, ShellRunner, ShellService};
use kiri_core::trace::NoopTraceSink;
use kiri_core::wire::WireRequest;
use kiri_core::zc_ipc_gate::ZcIpcGate;
use serde_json::json;

struct StubHttpClient;
impl HttpClient for StubHttpClient {
    fn fetch(&self, _req: HttpRequest) -> kiri_core::error::Result<HttpResponse> {
        Ok(HttpResponse { status: 200, headers: vec![], body: b"ok".to_vec() })
    }
}

struct StubShellRunner;
impl ShellRunner for StubShellRunner {
    fn run(&self, _p: &str, _a: &[String]) -> kiri_core::error::Result<ShellOutput> {
        Ok(ShellOutput { exit_code: 0, stdout: vec![], stderr: vec![] })
    }
}

fn host_allowlist() -> HostAllowlist {
    HostAllowlist::new(vec!["api.example.com".to_string()])
}

fn shell_allowlist() -> ShellAllowlist {
    ShellAllowlist::new(vec![AllowedCommand {
        program: "echo".to_string(),
        args: vec!["kiri-probe".to_string()],
    }])
}

fn router() -> Router {
    Router::new()
        .with_http(HttpService::new(Arc::new(StubHttpClient), host_allowlist(), Limits::default()))
        .with_shell(ShellService::new(
            Arc::new(StubShellRunner),
            shell_allowlist(),
            Limits::default(),
        ))
}

/// The production-shape gate: the double-gated surfaces declare their second
/// gate in ONE place. HTTP uses the arg-level host allowlist predicate,
/// SHELL_RUN uses the command allowlist predicate.
fn gate() -> ZcIpcGate {
    let mut g = ZcIpcGate::new();
    for id in [
        command_id::HTTP_GET,
        command_id::HTTP_POST,
        command_id::HTTP_PUT,
        command_id::HTTP_PATCH,
        command_id::HTTP_DELETE,
    ] {
        g.admit(id, kiri_core::zc_ipc_gate::http_host_admission(host_allowlist()));
    }
    g.admit(
        command_id::SHELL_RUN,
        kiri_core::zc_ipc_gate::shell_command_admission(shell_allowlist()),
    );
    g
}

fn caps(bit: u32) -> CapabilityBits {
    let mut c = CapabilityBits::empty();
    c.set(bit);
    c
}

const T0: u64 = 1_000_000_000;

fn http_req(url: &str) -> WireRequest {
    WireRequest::new(command_id::HTTP_GET, 1, 1, json!({ "url": url }))
}

// Property 1: missing capability DENIES even when the allowlist admits.
#[test]
fn missing_capability_denies_even_when_allowlist_admits() {
    let router = router();
    let mut gate = gate();
    let err = gate
        .mint(
            &router,
            CallerId(1),
            &CapabilityBits::empty(),
            &http_req("http://api.example.com/x"),
            T0,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthorized);
}

// Property 2: empty host allowlist DENIES even with the capability granted.
#[test]
fn empty_host_allowlist_denies_even_with_capability() {
    let router = router();
    let mut gate = ZcIpcGate::new();
    gate.admit(
        command_id::HTTP_GET,
        kiri_core::zc_ipc_gate::http_host_admission(HostAllowlist::new(vec![])),
    );
    let err = gate
        .mint(
            &router,
            CallerId(1),
            &caps(capability_bit::HTTP),
            &http_req("http://api.example.com/x"),
            T0,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ScopeDenied);
}

// Property 2b: an allowlisted surface whose host surface key was never
// allowed DENIES even with the capability granted.
#[test]
fn unadmitted_surface_key_denies_even_with_capability() {
    let router = router();
    let mut gate = ZcIpcGate::new();
    gate.admit_keyed(command_id::SHELL_RUN, "shell.allowlisted");
    let req = WireRequest::new(command_id::SHELL_RUN, 1, 1, json!({"program": "echo"}));
    let err = gate.mint(&router, CallerId(1), &caps(capability_bit::SHELL), &req, T0).unwrap_err();
    assert_eq!(err.code, ErrorCode::ScopeDenied);
}

// Property 3: both gates present ALLOWS (mint, redeem, gated reply bytes).
#[test]
fn both_gates_allow_mint_redeem_and_reply() {
    let router = router();
    let mut gate = gate();
    let req = http_req("http://api.example.com/x");
    let permit = gate
        .mint(&router, CallerId(1), &caps(capability_bit::HTTP), &req, T0)
        .expect("mint must succeed when bit + allowlist hold");
    let grant = gate.redeem(&permit, CallerId(1), &req, T0).expect("redeem");
    assert_eq!(grant.caller(), CallerId(1));
    assert_eq!(grant.command_id(), command_id::HTTP_GET);
    let bytes = b"shared-buffer-payload";
    assert_eq!(grant.reply_bytes(CallerId(1), command_id::HTTP_GET, bytes, T0), Some(&bytes[..]));
}

// Property 3b: the host-declared surface key path also mints once the host
// allows the surface key (used when args are not yet decoded).
#[test]
fn surface_key_mints_after_host_allows_surface() {
    let router = router();
    let mut gate = ZcIpcGate::new();
    gate.admit_keyed(command_id::SHELL_RUN, "shell.allowlisted");
    gate.allow_surface_key("shell.allowlisted");
    let req = WireRequest::new(command_id::SHELL_RUN, 2, 1, json!({"program": "echo"}));
    gate.mint(&router, CallerId(1), &caps(capability_bit::SHELL), &req, T0)
        .expect("surface-keyed mint must succeed once the host allows the key");
}

// Property 4: unknown command ids DENY (no PING fallback in the new gate).
#[test]
fn unknown_command_id_denies() {
    let router = router();
    let mut gate = gate();
    let req = WireRequest::new(999, 1, 1, json!(null));
    let err = gate.mint(&router, CallerId(1), &caps(capability_bit::PING), &req, T0).unwrap_err();
    assert_eq!(err.code, ErrorCode::ProtocolError);
}

// Property 5a: a forged token is not in the issued table and DENIES.
#[test]
fn forged_permit_denies() {
    let mut gate = gate();
    let forged = kiri_core::zc_ipc_gate::ZcIpcPermit::__test_only_forged(0xDEAD_BEEF);
    let err =
        gate.redeem(&forged, CallerId(1), &http_req("http://api.example.com/x"), T0).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthorized);
}

// Property 5b: a permit minted for one caller cannot be redeemed by another.
#[test]
fn wrong_caller_permit_denies() {
    let router = router();
    let mut gate = gate();
    let req = http_req("http://api.example.com/x");
    let permit = gate.mint(&router, CallerId(1), &caps(capability_bit::HTTP), &req, T0).unwrap();
    let err = gate.redeem(&permit, CallerId(2), &req, T0).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthorized);
}

// Property 5c: a permit minted for one command cannot be redeemed for another.
#[test]
fn wrong_command_permit_denies() {
    let router = router();
    let mut gate = gate();
    let req = http_req("http://api.example.com/x");
    let permit = gate.mint(&router, CallerId(1), &caps(capability_bit::HTTP), &req, T0).unwrap();
    let other =
        WireRequest::new(command_id::HTTP_POST, 1, 1, json!({"url": "http://api.example.com/x"}));
    let err = gate.redeem(&permit, CallerId(1), &other, T0).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthorized);
}

// Property 5d: a permit minted against one payload cannot be redeemed with
// rebound args (the args binding is part of the permit).
#[test]
fn rebound_args_permit_denies() {
    let router = router();
    let mut gate = gate();
    let req = http_req("http://api.example.com/x");
    let permit = gate.mint(&router, CallerId(1), &caps(capability_bit::HTTP), &req, T0).unwrap();
    let mutated = http_req("http://api.example.com/admin");
    let err = gate.redeem(&permit, CallerId(1), &mutated, T0).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthorized);
}

// Property 5e: an expired permit DENIES.
#[test]
fn expired_permit_denies() {
    let router = router();
    let mut gate = ZcIpcGate::with_ttl_ns(1_000);
    gate.admit(command_id::HTTP_GET, kiri_core::zc_ipc_gate::http_host_admission(host_allowlist()));
    let req = http_req("http://api.example.com/x");
    let permit = gate.mint(&router, CallerId(1), &caps(capability_bit::HTTP), &req, T0).unwrap();
    let err = gate.redeem(&permit, CallerId(1), &req, T0 + 1_001).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthorized);
}

// Property 5f: permits are single use; a replayed redeem DENIES.
#[test]
fn replayed_permit_denies() {
    let router = router();
    let mut gate = gate();
    let req = http_req("http://api.example.com/x");
    let permit = gate.mint(&router, CallerId(1), &caps(capability_bit::HTTP), &req, T0).unwrap();
    gate.redeem(&permit, CallerId(1), &req, T0).unwrap();
    let err = gate.redeem(&permit, CallerId(1), &req, T0).unwrap_err();
    assert_eq!(err.code, ErrorCode::Unauthorized);
}

// Property 6a: the unified pipe helper refuses to dispatch without a valid
// gate decision: denial produces an error response and NO grant, so the
// shared-buffer reply leg cannot be reached.
#[test]
fn pipe_helper_refuses_dispatch_without_permit() {
    let router = router();
    let mut gate = gate();
    let out = gate.dispatch_through_webview(
        &router,
        CallerId(1),
        &CapabilityBits::empty(),
        &http_req("http://api.example.com/x"),
        &mut NoopTraceSink,
        T0,
    );
    assert!(out.response.error.is_some());
    assert!(out.grant.is_none(), "no grant may exist when the gate denies");
}

// Property 6b: the shared-buffer reply leg fails closed without a live grant:
// wrong caller, wrong command, or expired grant yields no bytes to post.
#[test]
fn shared_buffer_reply_leg_requires_live_grant() {
    let router = router();
    let mut gate = ZcIpcGate::with_ttl_ns(1_000);
    gate.admit(command_id::HTTP_GET, kiri_core::zc_ipc_gate::http_host_admission(host_allowlist()));
    let req = http_req("http://api.example.com/x");
    let permit = gate.mint(&router, CallerId(1), &caps(capability_bit::HTTP), &req, T0).unwrap();
    let grant = gate.redeem(&permit, CallerId(1), &req, T0).unwrap();
    let bytes = b"bulk";
    assert!(grant.reply_bytes(CallerId(2), command_id::HTTP_GET, bytes, T0).is_none());
    assert!(grant.reply_bytes(CallerId(1), command_id::HTTP_POST, bytes, T0).is_none());
    assert!(grant.reply_bytes(CallerId(1), command_id::HTTP_GET, bytes, T0 + 1_001).is_none());
}

// End to end: the pipe helper dispatches a double-gated command only when
// BOTH gates pass, and the returned grant carries the reply-leg authority.
#[test]
fn through_webview_dispatch_is_double_gated_end_to_end() {
    let router = router();
    let mut gate = gate();
    let mut sink = NoopTraceSink;

    // allowlisted host + granted capability: dispatch runs, grant exists.
    let out = gate.dispatch_through_webview(
        &router,
        CallerId(1),
        &caps(capability_bit::HTTP),
        &http_req("http://api.example.com/x"),
        &mut sink,
        T0,
    );
    assert!(out.response.error.is_none(), "expected ok: {:?}", out.response.error);
    assert!(out.grant.is_some());

    // allowlist rejects the host: the gate denies before dispatch.
    let out = gate.dispatch_through_webview(
        &router,
        CallerId(1),
        &caps(capability_bit::HTTP),
        &http_req("http://evil.example.net/x"),
        &mut sink,
        T0,
    );
    assert!(out.response.error.is_some());
    assert!(out.grant.is_none());

    // allowlist would admit but capability missing: the gate denies.
    let out = gate.dispatch_through_webview(
        &router,
        CallerId(1),
        &CapabilityBits::empty(),
        &http_req("http://api.example.com/x"),
        &mut sink,
        T0,
    );
    assert!(out.response.error.is_some());
    assert!(out.grant.is_none());
}

// A capability-only surface (no declared second gate) mints with just the
// bit, matching today's single-gate behavior for those surfaces.
#[test]
fn capability_only_surface_mints_with_bit_alone() {
    let router = router();
    let mut gate = gate();
    let req = WireRequest::new(command_id::PING, 1, 1, json!(null));
    gate.mint(&router, CallerId(1), &caps(capability_bit::PING), &req, T0)
        .expect("capability-only surface must mint with its bit");
}

// The surface-level mint (args not yet decoded) cannot satisfy an args-level
// second gate; it fails closed rather than skipping the allowlist.
#[test]
fn args_surface_cannot_mint_without_decoded_args() {
    let router = router();
    let mut gate = gate();
    let err = gate
        .mint_surface(&router, CallerId(1), &caps(capability_bit::HTTP), command_id::HTTP_GET, T0)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::ScopeDenied);
}
