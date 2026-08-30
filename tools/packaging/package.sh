#!/usr/bin/env bash
# Kiri unsigned release packaging (G-3).
#
# Native OS signing is intentionally out of scope: Kiri has no Apple
# Developer account and this pipeline does not attempt codesign, notarization,
# or Windows Authenticode. Every artifact is labeled and published unsigned.
# Kiri's application-level Ed25519 update signature remains enabled: it binds
# the public artifact URL and SHA-256 of the exact bytes shipped.
#
# This script is headless and never launches kiri-host or a baseline binary.
#
# Outputs in OUT_DIR (default: artifacts):
#   macOS:   kiri-<version>-darwin-<arch>.zip + .dmg containing unsigned Kiri.app
#   Windows: kiri-<version>-windows-<arch>.zip containing kiri-host.exe
#   Linux:   kiri-<version>-linux-<arch>.tar.gz containing kiri-host
#   all OS:  RELEASES.json with a pinned-key signature over the artifact bytes

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
# shellcheck source=tools/packaging/lib-icon.sh
source "$ROOT/tools/packaging/lib-icon.sh"

OUT_DIR="${OUT_DIR:-artifacts}"
mkdir -p "$OUT_DIR"

if [ -z "${KIRI_UPDATE_SIGNING_KEY_HEX:-}" ]; then
  echo "KIRI_UPDATE_SIGNING_KEY_HEX is required to emit RELEASES.json" >&2
  echo "Store it in a secret manager; never commit the private update key." >&2
  exit 2
fi
if [ "$KIRI_UPDATE_SIGNING_KEY_HEX" = \
  "0707070707070707070707070707070707070707070707070707070707070707" ] && \
  [ "${KIRI_ALLOW_TEST_UPDATE_KEY:-}" != "1" ]; then
  echo "the deterministic test update key cannot publish a release" >&2
  echo "use a fresh private key whose public half matches the host-pinned key" >&2
  exit 2
fi

OS="$(uname -s)"
ARCH="$(uname -m)"
case "$OS" in
  Darwin)
    PLATFORM_OS="darwin"
    case "$ARCH" in
      arm64|aarch64) PLATFORM_ARCH="aarch64" ;;
      x86_64|amd64) PLATFORM_ARCH="x86_64" ;;
      *) echo "unsupported macOS architecture: $ARCH" >&2; exit 2 ;;
    esac
    ;;
  MINGW*|MSYS*|CYGWIN*|Windows_NT)
    PLATFORM_OS="windows"
    case "$ARCH" in
      x86_64|amd64) PLATFORM_ARCH="x86_64" ;;
      arm64|aarch64) PLATFORM_ARCH="aarch64" ;;
      *) echo "unsupported Windows architecture: $ARCH" >&2; exit 2 ;;
    esac
    ;;
  Linux)
    PLATFORM_OS="linux"
    case "$ARCH" in
      x86_64|amd64) PLATFORM_ARCH="x86_64" ;;
      aarch64|arm64) PLATFORM_ARCH="aarch64" ;;
      *) echo "unsupported Linux architecture: $ARCH" >&2; exit 2 ;;
    esac
    ;;
  *)
    echo "unsupported packaging host: $OS" >&2
    exit 2
    ;;
esac

PLATFORM_KEY="$PLATFORM_OS-$PLATFORM_ARCH"
PKG_VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)".*/\1/')"
BIN="kiri-host"
BIN_PATH="target/release/$BIN"
if [ "$PLATFORM_OS" = "windows" ]; then
  BIN_FILE="$BIN_PATH.exe"
else
  BIN_FILE="$BIN_PATH"
fi
ARTIFACT_STEM="kiri-$PKG_VERSION-$PLATFORM_KEY"

# ---------------------------------------------------------------------------
# OS signing readiness (native signing is out of scope for G-3 unsigned)
# ---------------------------------------------------------------------------
# Native OS signing (Apple codesign + notarization, Windows Authenticode) is
# intentionally absent: Kiri has no Apple Developer account and the pipeline
# does not attempt codesign. This stub is ready to accept certificates via
# APPLE_CERT / WINDOWS_CERT without changing the Ed25519 application-level
# signature. When certs are absent we emit an unsigned artifact; set
# KIRI_ALLOW_UNSIGNED=0 to require a cert and fail fast.
if [ -n "${APPLE_CERT:-}" ] || [ -n "${WINDOWS_CERT:-}" ]; then
  echo "OS signing certificate detected (APPLE_CERT/WINDOWS_CERT) — stub: native signing not yet implemented" >&2
  echo "Continuing with unsigned artifact; wire codesign/signtool here when certs are provisioned" >&2
else
  echo "OS signing not configured — emitting unsigned artifact (production requires certs)" >&2
  if [ "${KIRI_ALLOW_UNSIGNED:-1}" != "1" ]; then
    echo "KIRI_ALLOW_UNSIGNED=0 but no OS signing cert provided (APPLE_CERT/WINDOWS_CERT)" >&2
    exit 2
  fi
fi

echo "==> Kiri unsigned packaging ($OS/$ARCH, version $PKG_VERSION)"

# ---------------------------------------------------------------------------
# 0. Headless correctness gate. No native host or baseline is launched here.
# ---------------------------------------------------------------------------
echo "==> gate: fmt"
cargo fmt --all -- --check
echo "==> gate: clippy (native runtime target)"
cargo clippy -p kiri-runtime --all-targets -- -D warnings
echo "==> gate: workspace tests"
cargo test --workspace --quiet

# ---------------------------------------------------------------------------
# 1. Build the release binary.
# ---------------------------------------------------------------------------
echo "==> build release binary"
EMBED="${KIRI_EMBED_FRONTEND:-$ROOT/examples/starter}"
if [ ! -f "$EMBED/index.html" ]; then
  echo "KIRI_EMBED_FRONTEND has no index.html: $EMBED" >&2
  exit 2
fi
EMBED="$(cd "$EMBED" && pwd)"
echo "==> packing frontend $EMBED"
KIRI_EMBED_FRONTEND="$EMBED" cargo build --release -p kiri-runtime --bin "$BIN"

# ---------------------------------------------------------------------------
# 2. Assemble an unsigned, runnable archive for the current OS.
# ---------------------------------------------------------------------------
case "$PLATFORM_OS" in
  darwin)
    APP_DIR="$OUT_DIR/Kiri.app"
    ARTIFACT_PATH="$OUT_DIR/$ARTIFACT_STEM.zip"
    DMG_PATH="$OUT_DIR/$ARTIFACT_STEM.dmg"
    rm -rf "$APP_DIR" "$ARTIFACT_PATH" "$DMG_PATH"
    mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
    cp "$BIN_FILE" "$APP_DIR/Contents/MacOS/$BIN"
    make_icns "$ROOT/assets/kiri.png" "$APP_DIR/Contents/Resources/kiri.icns" 2>/dev/null || true
    sed "s/@KIRI_VERSION@/$PKG_VERSION/g" tools/packaging/Info.plist \
      > "$APP_DIR/Contents/Info.plist"
    ditto -c -k --keepParent "$APP_DIR" "$ARTIFACT_PATH"
    hdiutil create -volname Kiri -srcfolder "$APP_DIR" -ov -format UDZO "$DMG_PATH"
    ;;
  windows)
    STAGE_DIR="$OUT_DIR/$ARTIFACT_STEM"
    ARTIFACT_PATH="$OUT_DIR/$ARTIFACT_STEM.zip"
    rm -rf "$STAGE_DIR" "$ARTIFACT_PATH"
    mkdir -p "$STAGE_DIR"
    cp "$BIN_FILE" "$STAGE_DIR/$BIN.exe"
    cat > "$STAGE_DIR/run.cmd" <<'EOF'
@echo off
cd /d "%~dp0"
kiri-host.exe
EOF
    if command -v powershell.exe >/dev/null 2>&1 && command -v cygpath >/dev/null 2>&1; then
      WIN_STAGE_DIR="$(cygpath -w "$STAGE_DIR")"
      WIN_ARTIFACT_PATH="$(cygpath -w "$ARTIFACT_PATH")"
      powershell.exe -NoProfile -NonInteractive -Command \
        "Compress-Archive -Path '$WIN_STAGE_DIR\\*' -DestinationPath '$WIN_ARTIFACT_PATH' -Force"
    elif command -v zip >/dev/null 2>&1; then
      (cd "$STAGE_DIR" && zip -q -r "../$ARTIFACT_STEM.zip" .)
    else
      echo "Windows packaging requires powershell.exe/cygpath or zip" >&2
      exit 2
    fi
    ;;
  linux)
    STAGE_DIR="$OUT_DIR/$ARTIFACT_STEM"
    ARTIFACT_PATH="$OUT_DIR/$ARTIFACT_STEM.tar.gz"
    rm -rf "$STAGE_DIR" "$ARTIFACT_PATH"
    mkdir -p "$STAGE_DIR"
    cp "$BIN_FILE" "$STAGE_DIR/$BIN"
    cat > "$STAGE_DIR/run.sh" <<'EOF'
#!/usr/bin/env bash
cd "$(dirname "$0")"
exec ./kiri-host
EOF
    chmod +x "$STAGE_DIR/run.sh"
    tar -czf "$ARTIFACT_PATH" -C "$STAGE_DIR" "$BIN" run.sh
    ;;
esac

if [ ! -s "$ARTIFACT_PATH" ]; then
  echo "packaging produced no artifact: $ARTIFACT_PATH" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# 3. Emit a real signed manifest. The default URL is the predictable GitHub
#    release URL; KIRI_RELEASE_ASSET_URL overrides it for another publisher.
# ---------------------------------------------------------------------------
RELEASE_BASE_URL="${KIRI_RELEASE_BASE_URL:-https://github.com/ChloeVPin/kiri/releases/download/v$PKG_VERSION}"
ASSET_URL="${KIRI_RELEASE_ASSET_URL:-$RELEASE_BASE_URL/$(basename "$ARTIFACT_PATH")}"
echo "==> release manifest (Ed25519 over real artifact bytes)"
cargo run -q --release -p kiri-core --example emit_release_manifest -- \
  "$PKG_VERSION" "$PLATFORM_KEY" "$ASSET_URL" "$ARTIFACT_PATH" \
  "$OUT_DIR/RELEASES.json"

echo "==> unsigned artifact: $ARTIFACT_PATH"
echo "==> manifest: $OUT_DIR/RELEASES.json"
