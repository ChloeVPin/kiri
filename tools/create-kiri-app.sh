#!/usr/bin/env bash
# Copy the Kiri starter frontend into a new folder.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${1:-}"

if [ -z "$DEST" ] || [ "$DEST" = "-h" ] || [ "$DEST" = "--help" ]; then
  echo "usage: create-kiri-app.sh DIR"
  echo "  copies examples/starter into DIR/frontend plus a README"
  exit 2
fi

if [ -e "$DEST" ] && [ ! -d "$DEST" ]; then
  echo "not a directory: $DEST" >&2
  exit 2
fi

mkdir -p "$DEST/frontend"
cp -R "$ROOT/examples/starter/." "$DEST/frontend/"
rm -f "$DEST/frontend/README.md"

ABS_DEST="$(cd "$DEST" && pwd)"
cat > "$DEST/README.md" <<EOF
# $(basename "$ABS_DEST")

Kiri app frontend. Pack it into the host from the Kiri repo:

    cd $ROOT
    ./tools/packaging/make-app.sh --frontend $ABS_DEST/frontend
    open artifacts/Kiri.app

Or run the host without a bundle:

    KIRI_EMBED_FRONTEND=$ABS_DEST/frontend cargo build --release -p kiri-runtime --bin kiri-host
    ./target/release/kiri-host
EOF

echo "created $DEST"
echo "  frontend: $DEST/frontend"
echo "  next: $ROOT/tools/packaging/make-app.sh --frontend $ABS_DEST/frontend"
