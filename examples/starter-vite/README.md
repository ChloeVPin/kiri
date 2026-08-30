# starter-vite

Vite-based starter for Kiri. `npm create vite@latest` + `npm install` then set `KIRI_EMBED_FRONTEND` to the Vite `dist` output, or point the scaffold at this template.

```
./tools/create-kiri-app.sh --template starter-vite ~/Desktop/my-kiri-vite
KIRI_EMBED_FRONTEND="$PWD/examples/starter-vite" cargo build --release -p kiri-runtime --bin kiri-host
```

The host still serves `kiri://localhost` — Vite builds to static files, Kiri packs them.
