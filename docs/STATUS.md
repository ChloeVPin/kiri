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

The current local workspace gate passes 279 tests (220 `kiri-core`, 2
integration, and 57 `kiri-runtime`), formatting, native clippy, Windows target
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

The published v0.1.6 release contains the launcher-bearing Linux and Windows
archives and the signed three-platform manifest. Earlier pre-launcher releases
predate these archive changes.

## Public release acceptance checklist

- [x] Three platform archive entries exist in the public v0.1.6 manifest.
- [x] Scaffolders require a signed platform asset and verify its SHA-256.
- [x] Future release CI rejects archives without platform launchers.
- [x] Local packaging with the production Keychain key emits a verifiable
  signed manifest.
- [x] Publish v0.1.6 with native filesystem watching and bounded WebSocket
  transport; live manifest and all three platform archives verified.
- [x] Verify the v0.1.6 POSIX scaffold and native Windows PowerShell scaffold
  paths in the correctness workflow.


Measured startup, IPC, binary-size, and bulk-data results are maintained in
[`COMPETITIVE_ANALYSIS.md`](COMPETITIVE_ANALYSIS.md) and
[`../benchmark/README.md`](../benchmark/README.md). Results must retain their
environment and measurement method.

The runtime now has native `notify` filesystem watching and bounded local
`ws://` WebSocket transport on the desktop hosts. The application-menu command
surface remains registered and allowlisted, but native menu rendering is still
open because its platform objects are event-loop-affine.

Correctness run `32730288096` and controlled performance run `32730288110`
completed successfully across the hosted desktop matrix. The performance
artifacts retain complete Kiri/Tauri startup and IPC results; Windows Wry/Tao
startup remains explicitly incomplete after its bounded warmup timeout.

The corpus task queue currently marks T001-T008 and T010 complete, with T009
still in progress. T008's implementation and hosted evidence are summarized
in [`SHARED_BUFFER_REPORT.md`](SHARED_BUFFER_REPORT.md). Run `32696370579`
provides complete macOS and Windows Kiri/Tauri startup and IPC artifacts, but
the Windows Wry/Tao startup baseline is incomplete after its 20-second warmup
timeout, so T009 remains open for a stable three-way set.

## Open work

Open questions and their required evidence are maintained in
[`OPEN_QUESTIONS.md`](OPEN_QUESTIONS.md). Product gaps and comparative work are
maintained in [`GAP_MATRIX.md`](GAP_MATRIX.md).
