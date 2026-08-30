<!--
Thanks for contributing to Kiri. Keep the PR focused and verifiable.
-->

## Summary

What does this change do and why?

## Verification

- [ ] `cargo test --workspace` (279 tests)
- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy -p kiri-runtime --all-targets -- -D warnings`
- [ ] `cargo check --target x86_64-pc-windows-msvc -p kiri-runtime --all-targets`
- [ ] Native smoke/stress on your OS (`kiri-host --smoke`, `kiri-host-stress`)

## Platform impact

- [ ] Linux (wry/tao)
- [ ] macOS (wry/tao)
- [ ] Windows (Win32 + WebView2)
- [ ] No platform-specific behavior

## Security

Does this touch capability, allowlist, origin, resource, or IPC boundaries?

- [ ] No
- [ ] Yes — describe double-gating and tests:

## Docs

- [ ] Updated `docs/` / `README.md` if user-visible
- [ ] Added decision entry to `docs/DECISIONS.md` if architectural

## Linked issues / tasks

Closes # / T00X
