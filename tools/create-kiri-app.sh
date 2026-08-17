#!/usr/bin/env bash
# Scaffold a runnable Kiri app. Does not require this git tree: it can
# download the latest kiri-host and starter UI from GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/ChloeVPin/kiri/main/tools/create-kiri-app.sh | bash -s ./my-app
#   ./tools/create-kiri-app.sh ./my-app

set -euo pipefail

REPO="${KIRI_REPO:-ChloeVPin/kiri}"
DEST="${1:-}"

if [ -z "$DEST" ] || [ "$DEST" = "-h" ] || [ "$DEST" = "--help" ]; then
  echo "usage: create-kiri-app.sh DIR"
  echo "  builds a runnable app in DIR using the latest Kiri release"
  exit 2
fi

if [ -e "$DEST" ] && [ ! -d "$DEST" ]; then
  echo "not a directory: $DEST" >&2
  exit 2
fi

mkdir -p "$DEST"
ABS_DEST="$(cd "$DEST" && pwd)"
NAME="$(basename "$ABS_DEST")"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || true)"
LOCAL_STARTER=""
if [ -n "${SCRIPT_DIR}" ] && [ -f "$SCRIPT_DIR/../examples/starter/index.html" ]; then
  LOCAL_STARTER="$(cd "$SCRIPT_DIR/../examples/starter" && pwd)"
fi

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "need $1 on PATH" >&2
    exit 2
  }
}
need curl
need python3

echo "==> latest Kiri release"
FEED_URL="https://github.com/${REPO}/releases/latest/download/RELEASES.json"
FEED="$(curl -fsSL -A "create-kiri-app" "$FEED_URL")"
VERSION="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["version"])' <<<"$FEED")"
echo "    version $VERSION"

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
  Darwin)
    PLATFORM_OS="darwin"
    case "$ARCH" in
      arm64|aarch64) PLATFORM_ARCH="aarch64" ;;
      x86_64) PLATFORM_ARCH="x86_64" ;;
      *) echo "unsupported macOS arch: $ARCH" >&2; exit 2 ;;
    esac
    ASSET="kiri-${VERSION}-${PLATFORM_OS}-${PLATFORM_ARCH}.zip"
    ;;
  Linux)
    PLATFORM_OS="linux"
    case "$ARCH" in
      x86_64|amd64) PLATFORM_ARCH="x86_64" ;;
      aarch64|arm64) PLATFORM_ARCH="aarch64" ;;
      *) echo "unsupported Linux arch: $ARCH" >&2; exit 2 ;;
    esac
    ASSET="kiri-${VERSION}-${PLATFORM_OS}-${PLATFORM_ARCH}.tar.gz"
    ;;
  MINGW*|MSYS*|CYGWIN*)
    PLATFORM_OS="windows"
    PLATFORM_ARCH="x86_64"
    ASSET="kiri-${VERSION}-${PLATFORM_OS}-${PLATFORM_ARCH}.zip"
    ;;
  *)
    echo "unsupported OS: $OS" >&2
    exit 2
    ;;
esac

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ASSET_URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ASSET}"
echo "==> download $ASSET"
curl -fsSL -A "create-kiri-app" -o "$STAGE/$ASSET" "$ASSET_URL"

echo "==> frontend"
mkdir -p "$ABS_DEST/frontend"
if [ -n "$LOCAL_STARTER" ]; then
  cp -R "$LOCAL_STARTER/." "$ABS_DEST/frontend/"
  rm -f "$ABS_DEST/frontend/README.md"
else
  BASE="https://raw.githubusercontent.com/${REPO}/main/examples/starter"
  curl -fsSL -A "create-kiri-app" -o "$ABS_DEST/frontend/index.html" "$BASE/index.html"
  curl -fsSL -A "create-kiri-app" -o "$ABS_DEST/frontend/kiri.js" "$BASE/kiri.js"
  curl -fsSL -A "create-kiri-app" -o "$ABS_DEST/frontend/kiri.svg" "$BASE/kiri.svg"
fi

echo "==> assemble"
case "$PLATFORM_OS" in
  darwin)
    mkdir -p "$STAGE/unpack"
    ditto -x -k "$STAGE/$ASSET" "$STAGE/unpack"
    APP_SRC="$(find "$STAGE/unpack" -name 'Kiri.app' -type d | head -1)"
    if [ -z "$APP_SRC" ]; then
      echo "release zip did not contain Kiri.app" >&2
      exit 1
    fi
    APP="$ABS_DEST/${NAME}.app"
    rm -rf "$APP"
    cp -R "$APP_SRC" "$APP"
    mkdir -p "$APP/Contents/Resources/frontend"
    cp -R "$ABS_DEST/frontend/." "$APP/Contents/Resources/frontend/"
    /usr/libexec/PlistBuddy -c "Set :CFBundleName $NAME" "$APP/Contents/Info.plist" 2>/dev/null || true
    /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName $NAME" "$APP/Contents/Info.plist" 2>/dev/null || true
    echo "    open \"$APP\""
    ;;
  linux)
    mkdir -p "$ABS_DEST/bin"
    tar -xzf "$STAGE/$ASSET" -C "$STAGE"
    HOST="$(find "$STAGE" -name kiri-host -type f | head -1)"
    cp "$HOST" "$ABS_DEST/bin/kiri-host"
    chmod +x "$ABS_DEST/bin/kiri-host"
    cat > "$ABS_DEST/run.sh" <<EOF
#!/usr/bin/env bash
cd "\$(dirname "\$0")"
exec ./bin/kiri-host --frontend ./frontend
EOF
    chmod +x "$ABS_DEST/run.sh"
    echo "    $ABS_DEST/run.sh"
    ;;
  windows)
    mkdir -p "$ABS_DEST/bin"
    python3 - <<PY
import zipfile
z=zipfile.ZipFile("$STAGE/$ASSET")
z.extractall("$STAGE/unpack")
PY
    HOST="$(find "$STAGE/unpack" -name 'kiri-host.exe' -type f | head -1)"
    cp "$HOST" "$ABS_DEST/bin/kiri-host.exe"
    cat > "$ABS_DEST/run.cmd" <<EOF
@echo off
cd /d "%~dp0"
bin\\kiri-host.exe --frontend frontend
EOF
    echo "    $ABS_DEST/run.cmd"
    ;;
esac

cat > "$ABS_DEST/README.md" <<EOF
# $NAME

A Kiri app. Edit \`frontend/\` then run again.

    # macOS
    open ${NAME}.app

    # Linux
    ./run.sh

The host next to this folder is Kiri $VERSION. Your UI in \`frontend/\`
overrides the packed default. Ship from CI with the workflow in
https://github.com/${REPO}/blob/main/templates/ship-app.yml
EOF

echo "created $ABS_DEST (Kiri $VERSION)"
