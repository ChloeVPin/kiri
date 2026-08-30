---
name: Bug report
about: Something is broken on Linux, macOS or Windows
title: "[bug] "
labels: bug
assignees: ""
---

## Description

A clear description of the bug.

## Reproduction

**Kiri version:** (from `Cargo.toml` or `kiri-0.x.x` artifact)
**OS & backend:** (e.g. macOS wry/tao, Windows Win32+WebView2, Linux wry/tao)
**Frontend:** (`examples/blank` / `examples/demo` / custom)

Steps:

1. `cargo build -p kiri-runtime --bins`
2. `./target/debug/kiri-host --smoke --frontend ...`
3. ...

**Markers output:**

```
paste /tmp/kiri-startup.json or artifacts/startup.json
```

**Logs:**

```
paste relevant stderr / console output
```

## Expected behavior

## Screenshots / screencast (if UI)

## Additional context

- `cargo test --workspace` result:
- Does `examples/blank` reproduce it? (yes/no)
