# Kiri starter

A live host demo you can pack into `kiri-host`. It reads version, OS, arch,
and home through the real bridge, and can rename the window or copy a host
line to the clipboard.

```sh
# from the Kiri repo
KIRI_EMBED_FRONTEND="$PWD/examples/starter" cargo build --release -p kiri-runtime --bin kiri-host
./target/release/kiri-host
```

Or package a double-clickable Mac app:

```sh
./tools/packaging/make-app.sh --frontend examples/starter
open artifacts/Kiri.app
```

Copy this folder with `./tools/create-kiri-app.sh ~/Desktop/my-app` and edit
`frontend/` there. The host still posts the same startup ready markers as
`examples/blank`, so smoke and the marker schema stay valid.
