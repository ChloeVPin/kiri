# Kiri vs Tauri (honest, one page)

Searcher-facing summary. **Source of truth:**
[`COMPETITIVE_ANALYSIS.md`](COMPETITIVE_ANALYSIS.md)
(hosted scoreboard run `32730288110`). Do not invent numbers or cite
`bulk_bench` as user-facing IPC.

| Claim | Status |
|-------|--------|
| Same WebView engines | Yes: WebKit / WKWebView / WebView2. **No** render-speed claim. |
| Smaller unstripped host vs Tauri | ~3.6× (hosted macOS) / ~4.4× (hosted Windows) in the current size table; About’s “3.7×” matches the local macOS footprint claim in the scoreboard. |
| Faster startup always | **No.** Hosted medians flip across runs; do not claim a universal win. |
| Through-webview IPC | Often faster in published tables; not every payload (macOS 256 KiB is a counterexample). |
| Security model | Double-gate: capability **and** host-owned allowlist. JS never supplies the capability mask. See [`PRODUCT.md`](PRODUCT.md). |
| Ecosystem / mobile / store signing / plugin breadth | Tauri wins today. See [`GAP_MATRIX.md`](GAP_MATRIX.md). |

## When Kiri is the better bet

- You want a **stricter host authority** than “capability granted ⇒ native API open.”
- You care about a **smaller default desktop host** and are willing to trade Tauri’s plugin ecosystem.
- You ship **desktop only** (no mobile requirement).

## When Tauri is still the better bet

- You need mobile, a mature bundler + store signing path, or a large plugin surface.
- You need a polished `create-*-app` + docs-site ecosystem today.
- You need notarized / Authenticode-signed downloads without first-run OS warnings.

## Maturity (do not oversell)

Per [`PRODUCT.md`](PRODUCT.md), “done” requires: download/scaffold without owning
this git tree, a window without a terminal cheatsheet, embed-your-UI to three
OSes from CI, and a kept-current published scoreboard. Until those land, treat
releases as **evaluation / early development**.

Latest **published** host: [v0.1.6](https://github.com/ChloeVPin/kiri/releases/tag/v0.1.6).
Workspace version may already be ahead in `Cargo.toml`; that is not a download.

Try path: [`GETTING_STARTED.md`](GETTING_STARTED.md). Migrate map:
[`TEMPLATE_MIGRATION_TAURI.md`](TEMPLATE_MIGRATION_TAURI.md).
