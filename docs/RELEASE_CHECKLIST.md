# Release checklist (G-3 unsigned + signed-ready)

Unsigned artifacts are emitted with an Ed25519 `RELEASES.json`. Native OS signing
(Apple notarization, Windows Authenticode) is out of scope; the pipeline logs
`OS signing not configured — emitting unsigned artifact (production requires certs)`
and is ready to accept `APPLE_CERT` / `WINDOWS_CERT` via env vars when certs exist.
Set `KIRI_ALLOW_UNSIGNED=0` to require a cert and fail fast.

## 1. Build

```sh
cargo fmt --all -- --check
cargo test --workspace
# one platform at a time; artifacts are kiri-<version>-<os>-<arch>.*
KIRI_UPDATE_SIGNING_KEY_HEX="$(security find-generic-password -a $(id -un) -s kiri-update-signing-key -w)" \
  OUT_DIR=artifacts ./tools/packaging/package.sh
# For rehearsal without the production key (no publish):
KIRI_UPDATE_SIGNING_KEY_HEX=0707070707070707070707070707070707070707070707070707070707070707 \
  KIRI_ALLOW_TEST_UPDATE_KEY=1 KIRI_ALLOW_UNSIGNED=1 OUT_DIR=artifacts/rehearsal \
  bash tools/packaging/package.sh
```

Each run uses `tools/packaging/lib-icon.sh` and emits:
`kiri-<version>-darwin-<arch>.zip` + `.dmg`, `kiri-<version>-windows-<arch>.zip`,
`kiri-<version>-linux-<arch>.tar.gz`. The version must match `Cargo.toml`.

## 2. Verify manifest

```sh
# Single-platform self-check (already done by package.sh):
cat artifacts/RELEASES.json
# Cross-platform merge (CI does this on the 3-OS matrix):
python3 - <<'PY'
import hashlib, json, pathlib
m=json.loads(pathlib.Path("artifacts/RELEASES.json").read_text())
p, a = next(iter(m["platforms"].items()))
assert hashlib.sha256(pathlib.Path("artifacts/kiri-*.zip").read_bytes()).hexdigest() == a["sha256"]
PY
cargo run -q -p kiri-core --example verify_release_manifest -- \
  artifacts/RELEASES.json darwin-aarch64=artifacts/kiri-*.zip
# Must reject test key without KIRI_ALLOW_TEST_UPDATE_KEY=1 (package.sh exits 2).
```

The merged `RELEASES.json` is verified with `verify_release_manifest` against the
pinned public key `333d58ae…`. SHA-256 and archive contents (`Kiri.app`, `run.sh`,
`run.cmd`) are also checked.

## 3. Test scaffold

```sh
./tools/create-kiri-app.sh /tmp/kiri-scaffold-smoke
ls /tmp/kiri-scaffold-smoke/bin/kiri-host* /tmp/kiri-scaffold-smoke/frontend/index.html
# PowerShell on Windows:
# tools/create-kiri-app.ps1 $env:RUNNER_TEMP\kiri-scaffold-smoke
```

Scaffolder must verify `RELEASES.json` SHA-256 before extracting and must produce
a runnable launcher.

## 4. Check menu smoke

```sh
cargo build -p kiri-runtime --bins
./target/debug/kiri-host --smoke --frontend examples/menu-smoke --markers-out /tmp/kiri-menu.json
python3 -c "import json; d=json.load(open('/tmp/kiri-menu.json')); assert {'webview_ready','bridge_ready','dom_ready','first_animation_frame'} <= {m['name'] for m in d['markers']}"
# Windows CI runs the same via pwsh; Linux headless is a soft probe.
```

## 5. Publish

`unsigned-release.yml` runs on tag `v*`:

- `package` matrix builds unsigned artifact + signed manifest on each OS, rejects
  `070707…` test key unless `KIRI_ALLOW_TEST_UPDATE_KEY=1`, logs OS-signing stub.
- `merge-manifest` merges 3 manifests, checks SHA-256 + launchers, and verifies
  Ed25519 with the pinned key.
- `publish` (tag only) uploads `kiri-*.*` + `RELEASES.json` via `softprops/action-gh-release`.
- `rehearsal` (workflow_dispatch / push) builds on `ubuntu-latest` with the test key
  and verifies hash + Ed25519 without publishing.

To cut a release:

```sh
git tag v$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')
git push origin v$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')
```

CI creates the GitHub Release. Manual dispatch requires `release_base_url` (`https://…`).

