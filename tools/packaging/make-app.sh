#!/usr/bin/env bash
# Build an unsigned, double-clickable Kiri.app on this Mac.
#
# No signing key, no cargo test gate. The host binary plus examples/blank
# go in the standard bundle layout so kiri-host finds the frontend without
# --frontend. Native codesign/notarization is intentionally absent.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "make-app.sh is macOS-only. On Windows/Linux use tools/packaging/package.sh" >&2
  exit 2
fi

PKG_VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)".*/\1/')"
APP="${1:-$ROOT/artifacts/Kiri.app}"

echo "==> build release kiri-host"
cargo build --release -p kiri-runtime --bin kiri-host

echo "==> assemble $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/frontend"
cp target/release/kiri-host "$APP/Contents/MacOS/kiri-host"
cp -R examples/blank/. "$APP/Contents/Resources/frontend/"

if [ -f assets/kiri.png ] && command -v sips >/dev/null && command -v iconutil >/dev/null; then
  ICONSET="$(mktemp -d)/kiri.iconset"
  mkdir -p "$ICONSET"
  for size in 16 32 64 128 256 512; do
    sips -z "$size" "$size" assets/kiri.png --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" assets/kiri.png --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/kiri.icns"
  rm -rf "$(dirname "$ICONSET")"
elif [ -f assets/kiri.icns ]; then
  cp assets/kiri.icns "$APP/Contents/Resources/kiri.icns"
fi

sed "s/@KIRI_VERSION@/$PKG_VERSION/g" tools/packaging/Info.plist \
  > "$APP/Contents/Info.plist"

echo "==> unsigned app: $APP"
echo "    open \"$APP\""
echo "    or: \"$APP/Contents/MacOS/kiri-host\" --smoke"
