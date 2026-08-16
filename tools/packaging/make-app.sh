#!/usr/bin/env bash
# Build an unsigned, double-clickable Kiri.app on this Mac.
#
# The frontend is compiled into kiri-host (KIRI_EMBED_FRONTEND). The .app
# is just the binary + Info.plist + icon. No sidecar UI folder.
# Native codesign/notarization is intentionally absent.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [ "$(uname -s)" != "Darwin" ]; then
  echo "make-app.sh is macOS-only. On Windows/Linux use tools/packaging/package.sh" >&2
  exit 2
fi

FRONTEND="$ROOT/examples/starter"
APP="$ROOT/artifacts/Kiri.app"
while [ $# -gt 0 ]; do
  case "$1" in
    --frontend)
      FRONTEND="$2"
      shift 2
      ;;
    --out)
      APP="$2"
      shift 2
      ;;
    -h|--help)
      echo "usage: make-app.sh [--frontend DIR] [--out PATH.app]"
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [ ! -f "$FRONTEND/index.html" ]; then
  echo "frontend has no index.html: $FRONTEND" >&2
  exit 2
fi
FRONTEND="$(cd "$FRONTEND" && pwd)"

PKG_VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)".*/\1/')"

echo "==> embed $FRONTEND"
KIRI_EMBED_FRONTEND="$FRONTEND" cargo build --release -p kiri-runtime --bin kiri-host

echo "==> assemble $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/kiri-host "$APP/Contents/MacOS/kiri-host"

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
