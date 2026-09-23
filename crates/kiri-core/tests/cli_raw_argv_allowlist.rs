//! Second-gate regression lock for `kiri.cli.args` (command id 66).
//!
//! `CliService::describe` filtered `flags`/`options` through the host
//! allowlist but returned `parsed.raw` verbatim, so a frontend holding only
//! the CLI capability could still read an undeclared option (for example a
//! `--secret` launch token) through `payload.raw`. These tests dispatch
//! CLI_ARGS through a real Router with only the CLI capability granted and
//! assert the secret never appears in any returned field, with or without
//! `full: true`.

use kiri_core::caller::CallerId;
use kiri_core::capabilities::CapabilityBits;
use kiri_core::cli::{CliService, CLI_CAPABILITY};
use kiri_core::dispatch::{command_id, Router};
use kiri_core::trace::RingTraceSink;
use kiri_core::wire::WireRequest;
use serde_json::json;

fn argv() -> Vec<String> {
    ["app", "--secret=t0ps3cret", "--token", "hunter2", "--verbose", "--mode=fast", "run"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn cli_only_caps() -> CapabilityBits {
    let mut caps = CapabilityBits::empty();
    caps.set(CLI_CAPABILITY);
    caps
}

fn dispatch_args(
    router: &Router,
    caps: &CapabilityBits,
    payload: serde_json::Value,
) -> serde_json::Value {
    let resp = router.dispatch(
        CallerId(1),
        caps,
        &WireRequest::new(command_id::CLI_ARGS, 1, 1, payload),
        &mut RingTraceSink::new(16),
    );
    assert!(resp.error.is_none(), "cli.args failed: {:?}", resp.error);
    resp.payload.unwrap()
}

#[test]
fn cli_args_raw_never_contains_undeclared_secret() {
    let service = CliService::new(argv())
        .with_allowlist(vec!["verbose".to_string()], vec!["mode".to_string()]);
    let router = Router::new().with_cli(service);
    let caps = cli_only_caps();

    for payload in [json!(null), json!({ "full": true })] {
        let out = dispatch_args(&router, &caps, payload);
        let blob = serde_json::to_string(&out).unwrap();
        for needle in ["secret", "t0ps3cret", "token", "hunter2"] {
            assert!(!blob.contains(needle), "payload leaked {needle}: {blob}");
        }
        // Allowlisted entries still reach the structured fields.
        assert_eq!(out["flags"], json!(["verbose"]));
        assert_eq!(out["options"], json!({ "mode": "fast" }));
        assert!(blob.contains("--verbose"), "allowlisted flag missing: {blob}");
        assert!(blob.contains("--mode=fast"), "allowlisted option missing: {blob}");
    }
}

#[test]
fn cli_args_empty_allowlist_exposes_no_secret() {
    // Default CliService allowlist is empty: nothing beyond argv[0] and
    // positionals may be projected into raw.
    let service = CliService::new(argv());
    let router = Router::new().with_cli(service);
    let caps = cli_only_caps();

    for payload in [json!(null), json!({ "full": true })] {
        let out = dispatch_args(&router, &caps, payload);
        let blob = serde_json::to_string(&out).unwrap();
        for needle in ["secret", "t0ps3cret", "token", "hunter2", "verbose", "mode"] {
            assert!(!blob.contains(needle), "payload leaked {needle}: {blob}");
        }
        assert!(out["flags"].as_array().unwrap().is_empty());
        assert!(out["options"].as_object().unwrap().is_empty());
        assert_eq!(out["raw"], json!(["app", "run"]));
    }
}

#[test]
fn cli_args_still_denied_without_cli_capability() {
    let service = CliService::new(argv())
        .with_allowlist(vec!["verbose".to_string()], vec!["mode".to_string()]);
    let router = Router::new().with_cli(service);
    let resp = router.dispatch(
        CallerId(1),
        &CapabilityBits::empty(),
        &WireRequest::new(command_id::CLI_ARGS, 1, 1, json!(null)),
        &mut RingTraceSink::new(16),
    );
    assert!(resp.error.is_some(), "cli.args must stay capability-gated");
}
