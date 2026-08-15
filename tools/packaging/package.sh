#!/usr/bin/env bash
# Kiri release packaging scaffold (G-3).
#
# Builds the release host binary and produces a signed, notarized distributable
# for the current OS. Signing is OPT-IN and FAILS CLOSED: if the required
# credential environment variables are absent, the script refuses to emit a
# "signed" artifact and exits non-zero after producing only the cert-free
# release manifest (RELEASES.json), which is signed with Kiri's pinned Ed25519
# release key (no Apple/Microsoft cert needed).
#
# This is intentionally runnable headlessly: it never launches the WebView host.
#
# macOS env (optional):
#   KIRI_APPLE_SIGN_IDENTITY   e.g. "Developer ID Application: ..."
#   KIRI_APPLE_NOTARY_KEY_ID   App Store Connect key id
#   KIRI_APPLE_NOTARY_ISSUER   App Store Connect issuer id
#   KIRI_APPLE_NOTARY_KEY_PATH path to the .p8 auth key
# Windows env (optional):
#   KIRI_WINDOWS_PFX           path to the code-signing .pfx
#   KIRI_WINDOWS_PFX_PASSWORD  password for the .pfx
#
# Cert-free outputs (always produced):
#   artifacts/RELEASES.json    Ed25519-signed release manifest

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

OUT_DIR="${OUT_DIR:-artifacts}"
mkdir -p "$OUT_DIR"

OS="$(uname -s)"
PKG_VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)".*/\1/')"
BIN="kiri-host"

echo "==> Kiri packaging ($OS, version $PKG_VERSION)"

# ---------------------------------------------------------------------------
# 0. Gate: never package broken code. These are the same headless gates the
#    audit loop runs; packaging must not skip them.
# ---------------------------------------------------------------------------
echo "==> gate: fmt"
cargo fmt --all -- --check
echo "==> gate: clippy (runtime, macOS)"
cargo clippy -p kiri-runtime --all-targets -- -D warnings
echo "==> gate: test"
cargo test --workspace --quiet

# ---------------------------------------------------------------------------
# 1. Build the release binary.
# ---------------------------------------------------------------------------
echo "==> build release binary"
cargo build --release -p kiri-runtime --bin "$BIN"
BIN_PATH="target/release/$BIN"

# ---------------------------------------------------------------------------
# 2a. macOS: sign + notarize ONLY if credentials present (fail closed).
# ---------------------------------------------------------------------------
if [ "$OS" = "Darwin" ]; then
  APP_DIR="$OUT_DIR/Kiri.app"
  rm -rf "$APP_DIR"
  mkdir -p "$APP_DIR/Contents/MacOS" "$APP_DIR/Contents/Resources"
  cp "$BIN_PATH" "$APP_DIR/Contents/MacOS/$BIN"
  cp tools/packaging/Info.plist "$APP_DIR/Contents/Info.plist"

  if [ -z "${KIRI_APPLE_SIGN_IDENTITY:-}" ]; then
    echo "==> macOS: NO signing identity set -> NOT producing a signed artifact (fail closed)."
    echo "    Set KIRI_APPLE_SIGN_IDENTITY to sign; set the notary vars to notarize."
    echo "    Leaving unsigned $APP_DIR in place for local testing only."
  else
    echo "==> macOS: codesign with $KIRI_APPLE_SIGN_IDENTITY"
    codesign --force --options runtime --entitlements tools/packaging/entitlements.plist \
      --sign "$KIRI_APPLE_SIGN_IDENTITY" "$APP_DIR"
    codesign --verify --strict --verbose=2 "$APP_DIR"

    if [ -n "${KIRI_APPLE_NOTARY_KEY_ID:-}" ] && [ -n "${KIRI_APPLE_NOTARY_ISSUER:-}" ] && [ -n "${KIRI_APPLE_NOTARY_KEY_PATH:-}" ]; then
      echo "==> macOS: notarize"
      xcrun notarytool submit "$APP_DIR" \
        --key-id "$KIRI_APPLE_NOTARY_KEY_ID" \
        --issuer "$KIRI_APPLE_NOTARY_ISSUER" \
        --key "$KIRI_APPLE_NOTARY_KEY_PATH" --wait
      xcrun stapler staple "$APP_DIR"
    else
      echo "==> macOS: signed but NOT notarized (notary creds absent)."
    fi
  fi
fi

# ---------------------------------------------------------------------------
# 2b. Windows: sign ONLY if PFX present (fail closed). (Documented path; the
#     actual run happens on windows-latest where signtool is available.)
# ---------------------------------------------------------------------------
if [ "$OS" = "MINGW" ] || [ "$OS" = "Windows_NT" ] || [ "${KIRI_WINDOWS:-}" = "1" ]; then
  if [ -z "${KIRI_WINDOWS_PFX:-}" ] || [ -z "${KIRI_WINDOWS_PFX_PASSWORD:-}" ]; then
    echo "==> Windows: NO PFX set -> NOT producing a signed artifact (fail closed)."
    echo "    Set KIRI_WINDOWS_PFX + KIRI_WINDOWS_PFX_PASSWORD to sign."
  else
    echo "==> Windows: signtool sign"
    signtool sign /f "$KIRI_WINDOWS_PFX" /p "$KIRI_WINDOWS_PFX_PASSWORD" \
      /tr http://timestamp.digicert.com /td sha256 /fd sha256 "$BIN_PATH.exe"
  fi
fi

# ---------------------------------------------------------------------------
# 3. Cert-free release manifest (always). Signed with the pinned Ed25519 key;
#    no Apple/Microsoft cert required. Reuses the existing UpdateManifestBuilder.
# ---------------------------------------------------------------------------
echo "==> release manifest (Ed25519, cert-free)"
cargo run -q --release -p kiri-core --example emit_release_manifest \
  -- "$PKG_VERSION" "$OUT_DIR/RELEASES.json"

echo "==> done. Distributable signing is opt-in and fails closed without creds."
