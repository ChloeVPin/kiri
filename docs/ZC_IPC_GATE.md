# ZcIpcPermit: a fail-closed gate for the through-webview IPC pipe

Status: spike, proven by locking tests in
`crates/kiri-core/tests/zc_ipc_fail_closed_gate.rs` and unit tests in
`crates/kiri-core/src/zc_ipc_gate.rs`. Scope is the through-webview invoke
path and the shared-buffer reply leg only; no other surface was rewritten.

## The problem with today's shape

On the through-webview pipe (`handle_web_message` / `WebMessageReceived` on
Windows, `with_ipc_handler` on wry), authority for one request is decided by
two independent gates that live in different places:

1. The coarse capability bit, checked inside `Router::dispatch` via
   `validate::authorize_capability`, with the mask assigned natively by
   `security::trusted_frontend_capabilities()`.
2. The per-surface host allowlist, checked inside each command service
   (`HttpService::execute`, `ShellService::run`, ...), seeded from
   `host_policy.rs`.

A host wiring the pipe has to keep both in mind separately, and the
shared-buffer reply leg (`PostSharedBufferToScript`, T008) is not a
first-class gated object at all: it posts whatever `dispatch` produced.

## What was invented

`ZcIpcGate` collapses the two gates into one permit lifecycle on this pipe:

- `gate.mint(router, caller, caps, request, now)` issues an opaque, short
  lived, single-use `ZcIpcPermit` only when the command is registered, the
  caller holds the exact capability bits the router will enforce, AND the
  surface's declared second gate admits the payload. Every condition is
  required; there is no way to mint with only one side satisfied for a
  declared surface.
- `gate.redeem(permit, caller, request, now)` validates the token against
  the gate's issued table (caller, command, and args-digest binding plus
  expiry), consumes it, and returns a `ZcIpcGrant`.
- `grant.reply_bytes(caller, command_id, bytes, now)` is the only currency
  the reply leg accepts: the shared-buffer post releases bytes only under a
  live grant bound to the same caller and command.
- `gate.dispatch_through_webview(...)` runs mint, redeem, and
  `Router::dispatch` as one operation and returns a `GatedResponse` whose
  `grant` is `Some` only when both gates passed.

Permits are bound to (caller, command id, args digest) and expire after a
two-second TTL. Tokens come from an OS-seeded SplitMix64 stream; the issued
table is the authority, so a fabricated or replayed value denies.

Two second-gate flavors cover the whole pipe: `admit` runs an arg-level
predicate over the decoded payload (mirroring `HttpService` /
`ShellService`), and `admit_keyed` + `allow_surface_key` cover legs minted
before args decode via a host-declared surface key. Arg-level surfaces deny
at surface-mint rather than skipping their allowlist.

## Concrete comparison on this surface

For the through-webview invoke plus shared-buffer reply of `kiri.http.get`:

- Before: authority = `caller_caps` passed to `Router::dispatch` (bit gate
  in `validate.rs`) + `HostAllowlist` inside `HttpService` (second gate in
  `http.rs`) + nothing on `post_shared_buffer`. Three separate things a host
  had to wire and audit, in three files.
- After: authority = one `ZcIpcGate` declaration in `host_policy::zc_ipc_gate`
  next to the seed lists, one `dispatch_through_webview` call in each
  backend, and `reply_bytes` gating `post_shared_buffer`. The mental model is
  one permit lifecycle: no permit, nothing crosses the pipe in either
  direction.

Compared to the Tauri-shaped model (plugin capability bitset plus
per-plugin allow/scope lists), the difference on THIS surface is that the
two conditions are fused into a single unforgeable-by-construction object at
the pipe boundary instead of two booleans evaluated at different layers, and
the reply leg carries a gate where Tauri-shaped models gate only the invoke.

## Strictness evidence

Same checks, same inputs: the capability check reads `Router::required_bits`
(the exact bits the router will dispatch with) and the allowlist predicates
wrap the same `HostAllowlist` / `ShellAllowlist` values the services use.
In-service checks still run at execute time, so the gate can only ever add a
denial, never remove one. On top of that it is stricter at the edges:
unknown command ids fail closed against live router registration (no `PING`
fallback, aligned with PR #31 semantics), and the permit adds caller /
command / args binding, expiry, single-use redemption, and a reply-leg gate
that did not exist before.

Locking tests (all green): `missing_capability_denies_even_when_allowlist_admits`,
`empty_host_allowlist_denies_even_with_capability`,
`unadmitted_surface_key_denies_even_with_capability`,
`both_gates_allow_mint_redeem_and_reply`,
`surface_key_mints_after_host_allows_surface`, `unknown_command_id_denies`,
`forged_permit_denies`, `wrong_caller_permit_denies`,
`wrong_command_permit_denies`, `rebound_args_permit_denies`,
`expired_permit_denies`, `replayed_permit_denies`,
`pipe_helper_refuses_dispatch_without_permit`,
`shared_buffer_reply_leg_requires_live_grant`,
`through_webview_dispatch_is_double_gated_end_to_end`,
`capability_only_surface_mints_with_bit_alone`,
`args_surface_cannot_mint_without_decoded_args`.

## Limits of the claim

- The gate is in-process: the token is not a cross-process MAC and does not
  need to be, since the trust boundary is the webview message handler.
- Undeclared surfaces are capability-only on the pipe, matching today. Their
  in-service allowlists still run; folding them into the gate is additive
  follow-up work, not required for correctness.
- No performance claim is made. Benchmark Honesty owns scoreboard numbers;
  nothing here asserts hosted RTT or throughput changes.
