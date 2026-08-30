> **ARCHIVED — superseded by `docs/GAP_MATRIX.md` + `docs/ROADMAP.md`. Retained for history; do not update.**

# Kiri exceed-Tauri program

This is the working contract for the long-running effort to make Kiri a
faster and better alternative to Tauri for ordinary desktop application
developers. It supersedes the narrower “win only where winnable” framing in
older competitive notes. It does not permit unsupported universal claims:
each dimension below has a workload, comparison protocol, and acceptance
gate.

## Product outcome

An average developer can create, run, package, update, and debug a Kiri
application on Linux, macOS, or Windows with less friction than an equivalent
Tauri application, while the resulting application is at least as reliable
and secure and is measurably faster or more resource-efficient on the
specified workloads.

“Better in all aspects” means no known customer-critical dimension is
knowingly abandoned. It does not mean that Kiri must win every workload or
reproduce Tauri’s mobile product during the desktop-first phase.

## Non-negotiable invariants

- Linux, macOS, and Windows are equal release targets. Platform-specific
  implementation is acceptable; platform-specific neglect is not.
- A frontend request must pass both a capability check and a host-owned
  allowlist check. Performance work may not remove either check.
- Existing protocol ids, error categories, resource ownership checks, origin
  checks, path bounds, and backpressure remain compatible unless a versioned
  migration is documented.
- Benchmark wins do not count if they increase crash rate, timeout rate,
  memory exhaustion, security exposure, or maintenance burden beyond the
  accepted budget.
- A result is reported with raw samples, environment, revision, webview
  version, warm-up policy, workload, and uncertainty. A single favorable run
  is not a release claim.

## Acceptance gates

### A. Startup and launch reliability

For the same embedded frontend, release configuration, and equivalent host
settings, measure cold process launch, warm launch, WebView-ready, bridge-ready,
DOM-ready, and first animation frame on all three OS families.

- Kiri must beat or match Tauri at p50 and p95 for first animation frame on
  the reference workload on each OS family.
- Kiri must not exceed Tauri's timeout or non-zero-exit rate.
- A second workload with a larger frontend must be included so an optimized
  blank page cannot stand in for application startup.
- Persistent WebView profiles, GPU mode, asset mode, and OS power policy must
  be explicit and identical where the platform permits.

### B. IPC and data movement

Use a live WebView for request/response tests at 0 B, 64 B, 1 KiB, 16 KiB,
256 KiB, 1 MiB, and a streaming payload. Include sequential, concurrent, and
backpressured workloads.

- Kiri must beat Tauri on p50 and p95 round-trip latency for small and medium
  control messages, or document an intentional parity result with a simpler
  or safer contract.
- Kiri must beat Tauri on throughput and peak RSS for large payloads without
  falling back to an unbounded allocation path.
- Every payload must be byte-equivalent and every error/timeout must be
  counted.
- Shared-buffer, ordinary-message, and streaming paths must be separately
  labeled; no in-process router result may be presented as app IPC.

### C. Resource efficiency

Measure stripped release artifact size, installed size, cold peak RSS, warm
steady-state RSS, CPU during idle and active workloads, and launch I/O.

- Kiri must remain below Tauri on host binary and installed footprint for the
  reference desktop app, or the larger footprint must buy a documented,
  verified capability that users value.
- Kiri must not use materially more idle CPU or memory than Tauri on any
  supported OS family.
- Measurements must include the same frontend and equivalent symbols/signing
  posture.

### D. Security and isolation

Build an adversarial matrix covering every public command, malformed wire
input, wrong capability, missing allowlist, wrong caller, stale resource,
origin mismatch, path escape, oversized payload, plugin manifest mismatch,
and update signature failure.

- All unauthorized cases fail closed on Linux, macOS, and Windows.
- No benchmark optimization may bypass a security decision or move trust into
  frontend JavaScript.
- Kiri must provide a clearer, more reviewable least-privilege configuration
  than the equivalent Tauri example, with generated validation errors.

### E. Native capability coverage

Reach practical desktop parity for the capabilities an ordinary app needs:
windows, menus, tray, dialogs, clipboard, filesystem scopes, HTTP, WebSocket,
notifications, shortcuts, autostart, deep links, opener, sidecars, store,
CLI, events, updater, and diagnostics.

- Each capability has a typed Rust contract, a JavaScript binding, a host
  implementation on all desktop targets or an explicit platform result, an
  allowlist model, a negative test, and a working example.
- Platform differences are surfaced in capability metadata and errors rather
  than silently becoming no-ops.
- Plugin loading remains host-approved, manifest-validated, and default-deny.

### F. Developer experience

A clean machine with documented prerequisites must be able to scaffold an
application, install dependencies, run a development server, invoke a native
operation, build release artifacts, and recover from common configuration
errors without reading internal source code.

- Provide a first-party CLI or equivalent workflow with consistent commands
  on Linux, macOS, and Windows.
- Provide starter templates for at least a plain HTML app and one mainstream
  frontend integration, with typed API generation and useful diagnostics.
- Error messages identify the failed layer and the next corrective action.
- Documentation includes a Tauri migration path and a capability/security
  explanation for non-experts.

### G. Distribution and lifecycle

- Produce installable artifacts for Linux, macOS, and Windows with correct
  metadata, icons, launchers, uninstall behavior, and architecture labels.
- Support application-level signed update verification with a pinned public
  key, rollback-safe failure behavior, and a user-visible update flow.
- Add platform-native signing/notarization when credentials and policy permit;
  until then, document the exact unsigned limitation.
- CI must build, inspect, and smoke-test every published artifact on its
  target OS, including the scaffolded-app path.

### H. Accessibility and quality

- Keyboard navigation, focus behavior, semantic starter UI, reduced motion,
  high contrast, and screen-reader-friendly examples are verified in the
  starter experience.
- Public APIs have stable error semantics, versioning rules, and generated
  reference documentation.
- Repeated CI runs demonstrate no regression in tests, startup reliability,
  memory safety, or benchmark validity.

## Research and benchmark protocol

The comparison baseline is current Tauri 2 desktop documentation and source,
not an old version or a hand-written mock. Tauri documents capability files,
window/platform scoping, official plugins, WebView2 on Windows, WKWebView on
macOS, and WebKitGTK on Linux. These are part of the comparison surface, not
optional extras.

The first benchmark revision must define:

1. Reference hardware or hosted runner image per OS.
2. Tauri and Kiri revisions, Rust/toolchain versions, and WebView versions.
3. Embedded and development asset-delivery modes.
4. Cold and warm launch procedures, process cleanup, profile state, and
   sample counts.
5. p50, p95, p99 where useful, confidence/variance, failures, and resource
   measurements.
6. A representative application workload in addition to blank-page smoke.
7. A reproducible command and raw artifact format checked by CI.

The benchmark is successful only when an independent run can reproduce the
direction of the result. If it cannot, the claim is narrowed or rejected.

## Execution sequence

1. Freeze this contract and inventory the current implementation against it.
2. Research Tauri's current desktop feature, plugin, packaging, and security
   surface from primary sources; record only comparable claims.
3. Repair the benchmark harness and collect a fresh Kiri/Tauri baseline before
   optimizing anything.
4. Profile startup, IPC, asset delivery, allocations, and resource usage on
   each available OS; use hosted Windows/Linux runs for unavailable hardware.
5. Implement the highest-leverage bottleneck fix, one measurable change at a
   time, with focused regression tests.
6. Close ordinary-user workflow gaps: CLI, templates, errors, capabilities,
   packaging, updates, and migration docs.
7. Run security, accessibility, reliability, and cross-platform verification.
8. Publish only the subset of “faster/better” claims that the raw evidence
   supports, with the remaining gaps and revisit triggers recorded.

## Current evidence and known risks

The repository already has a double-gated control plane, cross-platform
startup markers, a signed manifest verifier, launcher-bearing v0.1.6 release
artifacts, and a broad native command catalog. Current evidence shows a real
Kiri IPC advantage in the hosted samples and a footprint advantage over the
Tauri baseline, but startup is workload- and runner-sensitive. Tauri still has
the stronger plugin catalog, CLI ergonomics, templates, and distribution
polish. The Windows Wry/Tao comparison also needs a stable measurement rather
than a single long sample.

The first disconfirming tests are therefore: representative embedded startup
on Windows, p95 IPC under concurrency, cold/warm memory on all OS families,
and a clean-machine scaffold-to-release workflow. If any fails, the plan is
revised from the measurement rather than hidden by changing the workload.

## Research references

- [Tauri capabilities](https://tauri.app/security/capabilities/)
- [Tauri WebView versions](https://tauri.app/reference/webview-versions/)
- [Tauri official features and plugins](https://v2.tauri.app/plugin/)
- [Tauri plugin development](https://tauri.app/develop/plugins/)
- [Tauri updater permissions](https://v2.tauri.app/es/plugin/updater/)
