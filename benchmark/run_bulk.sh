#!/usr/bin/env bash
# T007 ordinary-message bulk-path benchmark (Mac/Linux-runnable).
#
# Measures the kiri-core JSON control path (serialize WireRequest ->
# Router.dispatch -> deserialize WireResponse) at 1 MB / 16 MB / 100 MB,
# recording per-iteration wall clock, CPU time, and peak RSS. Emits a raw
# JSON artifact (full sample arrays, not just summaries) under artifacts/.
#
# This is the "ordinary message" path. The WebView2 shared-buffer fast path
# is a separate, Windows-gated experiment (T008).
set -euo pipefail
cd "$(dirname "$0")/.."

RUNS="${KIRI_BULK_RUNS:-20}"
OUT="${KIRI_BULK_OUT:-artifacts/bulk-ordinary.json}"
PROFILE="${KIRI_BULK_PROFILE:-release}"

echo "building kiri-core bulk_bench ($PROFILE)..."
cargo build --"$PROFILE" --example bulk_bench -p kiri-core

echo "running bulk benchmark: runs=$RUNS out=$OUT"
KIRI_BULK_RUNS="$RUNS" KIRI_BULK_OUT="$OUT" \
  ./"target/$PROFILE/examples/bulk_bench"

echo "wrote $OUT"
