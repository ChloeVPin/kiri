#!/usr/bin/env bash
# Wrap artifacts/Kiri.app in an unsigned UDZO disk image.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

APP="${1:-$ROOT/artifacts/Kiri.app}"
DMG="${2:-$ROOT/artifacts/Kiri.dmg}"

if [ ! -d "$APP" ]; then
  echo "missing app: $APP (run tools/packaging/make-app.sh first)" >&2
  exit 2
fi

mkdir -p "$(dirname "$DMG")"
rm -f "$DMG"
hdiutil create -volname Kiri -srcfolder "$APP" -ov -format UDZO "$DMG"
echo "==> unsigned dmg: $DMG"
