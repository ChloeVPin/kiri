# Project status

This document is the home for changing implementation status and verification
evidence. The README intentionally stays focused on the product, architecture,
and usage.

## Current implementation

Kiri has platform-native hosts for Linux, macOS, and Windows, with a shared
control protocol and security model. The runtime exposes capability-gated
native surfaces and host allowlists across the supported desktop platforms.

Exact command coverage is documented in [`API_REFERENCE.md`](API_REFERENCE.md).
The release scaffold now supports POSIX shells and native Windows
PowerShell, verifies the downloaded artifact SHA-256 against `RELEASES.json`,
and creates runnable launchers for Linux and Windows archives.
Linux desktop use still depends on the distribution's GTK 3 and WebKit2GTK
4.1 runtime libraries; the getting-started guide now states that prerequisite.
The correctness workflow syntax-checks the POSIX packaging tools on Unix
runners and parses the PowerShell scaffolder on Windows runners.

## Verification

The authoritative per-platform verification record is
[`CROSS_PLATFORM_STATUS.md`](CROSS_PLATFORM_STATUS.md). It records native
execution, cross-compilation, soft probes, and blocked hardware-dependent
checks separately.

The current local workspace gate passes 270 tests (218 `kiri-core`, 2
integration, and 50 `kiri-runtime`), formatting, native clippy, Windows target
check/clippy, and both standalone baseline checks. The native macOS build,
smoke run, and three-cycle stress run also pass. These results describe the
current worktree and should be refreshed after substantive changes.

Local archive assembly was inspected successfully, including the macOS app
layout and launcher-bearing archive changes. Emitting a publishable manifest
requires the private key whose public half matches the host-pinned update key;
the deterministic test key is rejected by the producer and is not a valid
release substitute.

The real Keychain-backed signing key was used for a local macOS packaging
rehearsal. The release producer verified the generated Ed25519 manifest against
the pinned public key, and the launcher-bearing macOS archive hash matched the
manifest. This validates the signing path without publishing the artifact.

The published v0.1.4 release contains the launcher-bearing Linux and Windows
archives and the signed three-platform manifest. The older v0.1.3 release
predates these launcher changes.

## Public release acceptance checklist

- [x] Three platform archive entries exist in the public v0.1.3 manifest.
- [x] Scaffolders require a signed platform asset and verify its SHA-256.
- [x] Future release CI rejects archives without platform launchers.
- [x] Local packaging with the production Keychain key emits a verifiable
  signed manifest.
- [x] Publish v0.1.4 containing the current launchers.
- [x] Verify v0.1.4 through the POSIX scaffold and native Windows PowerShell
  scaffold smoke test in correctness run `32686438971`.

Measured startup, IPC, binary-size, and bulk-data results are maintained in
[`COMPETITIVE_ANALYSIS.md`](COMPETITIVE_ANALYSIS.md) and
[`../benchmark/README.md`](../benchmark/README.md). Results must retain their
environment and measurement method.

The corpus task queue currently marks T001-T008 and T010 complete, with T009
still in progress. T008's implementation and hosted evidence are summarized
in [`SHARED_BUFFER_REPORT.md`](SHARED_BUFFER_REPORT.md). T009 requires the
remaining stable hosted comparison evidence before it can be called complete.

## Open work

Open questions and their required evidence are maintained in
[`OPEN_QUESTIONS.md`](OPEN_QUESTIONS.md). Product gaps and comparative work are
maintained in [`GAP_MATRIX.md`](GAP_MATRIX.md).
