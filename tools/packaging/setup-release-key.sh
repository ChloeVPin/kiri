#!/usr/bin/env bash
# Configure Kiri's application-level update key in GitHub without exposing it.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

SERVICE="kiri-update-signing-key"
ACCOUNT="$(id -un)"

if ! command -v gh >/dev/null 2>&1; then
  echo "GitHub CLI is missing. Install it from https://cli.github.com/" >&2
  exit 1
fi
if ! gh auth status --hostname github.com >/dev/null 2>&1; then
  echo "GitHub CLI is not authenticated. Run: gh auth login" >&2
  exit 1
fi
if ! security find-generic-password -a "$ACCOUNT" -s "$SERVICE" >/dev/null 2>&1; then
  echo "The local Keychain entry '$SERVICE' is missing." >&2
  exit 1
fi

remote="$(git config --get remote.origin.url || true)"
case "$remote" in
  https://github.com/*) repo="${remote#https://github.com/}" ;;
  git@github.com:*) repo="${remote#git@github.com:}" ;;
  *)
    echo "Cannot resolve a GitHub repository from origin: $remote" >&2
    exit 1
    ;;
esac
repo="${repo%.git}"

if gh secret list --repo "$repo" 2>/dev/null | awk '{print $1}' | grep -Fxq KIRI_UPDATE_SIGNING_KEY_HEX; then
  echo "GitHub secret already exists: KIRI_UPDATE_SIGNING_KEY_HEX"
else
  private_key="$(security find-generic-password -a "$ACCOUNT" -s "$SERVICE" -w)"
  printf '%s' "$private_key" | gh secret set KIRI_UPDATE_SIGNING_KEY_HEX --repo "$repo" >/dev/null
  unset private_key
  echo "GitHub secret created: KIRI_UPDATE_SIGNING_KEY_HEX"
fi

version="$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)".*/\1/')"
echo
echo "Everything is configured for $repo."
echo "Next, when you are ready to publish:"
echo "  git push origin main"
echo "  git tag v$version"
echo "  git push origin v$version"
echo
echo "The tag starts the unsigned macOS/Windows/Linux release workflow."
