# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

Kiri is pre-1.0. Security fixes are applied to the latest `main` and the
latest published release tag.

## Reporting a Vulnerability

**Do not open a public issue for security vulnerabilities.**

Email **chloevalesquez@gmail.com** with:

- Affected version / commit
- Reproduction steps or PoC
- Impact assessment (what attacker can do)

You will receive an acknowledgement within 72 hours. We will coordinate a
fix and disclosure timeline with you. If you do not hear back, please open a
minimal placeholder issue referencing that you sent an email (no details).

## Scope

- `crates/kiri-core` control-plane validation, capability / allowlist enforcement
- `crates/kiri-runtime` platform hosts (`host_cross.rs` Linux/macOS, `host_windows.rs` Windows)
- `tools/packaging/package.sh` and `tools/create-kiri-app.sh` / `.ps1` signature and hash verification
- `kiri://localhost` origin and resource handling

Out of scope: Tauri/Wry baselines in `baselines/`, example frontends in
`examples/` (unless they demonstrate a bypass of the host boundary).

## Disclosure

Once a fix is available on `main` and a patched release is published, we
will publish a GitHub Security Advisory (GHSA) crediting the reporter unless
anonymity is requested.

## Past Advisories

None published yet. See `docs/DECISIONS.md` and `docs/COMPETITIVE_ANALYSIS.md`
for the security model and double-gating guarantees.
