#!/usr/bin/env python3
"""macOS-native three-way startup-marker comparison.

Runs the native kiri-host (wry/tao backend), the standalone wry-tao baseline,
and the standalone Tauri baseline on the same machine and compares their
startup markers against the frozen schema (docs/research/markers-schema.md).

Per Q-003: only webview_ready-and-earlier phases are directly comparable across
all three targets. Tauri routes dom/frame markers through
__TAURI_INTERNALS__.invoke, which costs more than the wry window.ipc path, so
those phases are reported but flagged non-comparable.
"""
import json, os, subprocess, statistics, sys

ROOT = "/Users/chloe/Developer/kiri"
N = 5

TARGETS = {
    "NATIVE (kiri-host)": {
        "cmd": [f"{ROOT}/target/debug/kiri-host", "--smoke",
                "--frontend", f"{ROOT}/examples/blank", "--markers-out", "/tmp/k.json"],
        "out": "/tmp/k.json",
    },
    "WRY-TAO baseline": {
        "cmd": [f"{ROOT}/baselines/wry-tao/target/debug/wry-tao-baseline"],
        "out": "/tmp/wt.json",
    },
    "TAURI baseline": {
        "cmd": [f"{ROOT}/baselines/tauri/target/debug/tauri-baseline"],
        "out": "/tmp/ta.json",
    },
}

COMPARABLE = ["platform_initialized", "webview_creation_requested", "webview_ready"]
FLAGGED = ["bridge_ready", "dom_ready", "app_ready", "first_animation_frame"]


def grab(path):
    # Native host pretty-prints the markers file (multi-line), baselines emit a
    # single line. Accept both: try whole-file parse first, then line scan.
    text = open(path).read()
    try:
        d = json.loads(text)
        if isinstance(d, dict) and "markers" in d:
            return d
    except json.JSONDecodeError:
        pass
    for line in text.splitlines():
        if line.strip().startswith("{") and "schema_version" in line:
            try:
                return json.loads(line)
            except json.JSONDecodeError:
                continue
    return None


def run_once(spec):
    # Read from stdout when the target prints JSON there; fall back to the
    # markers-out file (used by the native host, which writes on exit).
    proc = subprocess.run(spec["cmd"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=True, text=True)
    d = grab_string(proc.stdout)
    if d is None and spec.get("out"):
        try:
            d = grab_string(open(spec["out"]).read())
        except OSError:
            d = None
    assert d is not None, f"no schema_version JSON for {spec['cmd']}"
    return {m["name"]: m["since_first_ns"] for m in d["markers"]}


def grab_string(text):
    if not text:
        return None
    try:
        d = json.loads(text)
        if isinstance(d, dict) and "markers" in d:
            return d
    except json.JSONDecodeError:
        pass
    for line in text.splitlines():
        if line.strip().startswith("{") and "schema_version" in line:
            try:
                return json.loads(line)
            except json.JSONDecodeError:
                continue
    return None


def med(xs):
    return statistics.median(xs)


results = {}
for label, spec in TARGETS.items():
    samples = []
    for _ in range(N):
        samples.append(run_once(spec))
    results[label] = samples

print(f"macOS-native startup comparison (median of {N} runs, ns since first marker)\n")
hdr = "marker".ljust(26) + "".join(l.split()[0].ljust(22) for l in TARGETS)
print(hdr)
for name in COMPARABLE:
    row = name.ljust(26)
    for label in TARGETS:
        vals = [s[name] for s in results[label]]
        row += f"{int(med(vals)):,}".ljust(22)
    print(row + "  [comparable]")
for name in FLAGGED:
    row = name.ljust(26)
    for label in TARGETS:
        vals = [s.get(name, None) for s in results[label]]
        vals = [v for v in vals if v is not None]
        row += f"{int(med(vals)):,}".ljust(22)
    tag = "  [TAURI non-comparable: invoke path]" if name in ("dom_ready", "app_ready", "first_animation_frame") else ""
    print(row + tag)

# persist raw for artifact
artifact = {"platform": "macOS aarch64", "runs": N, "targets": {k: v for k, v in results.items()}}
with open(f"{ROOT}/artifacts/compare-macos.json", "w") as f:
    json.dump(artifact, f, indent=2)
print("\nraw artifact -> artifacts/compare-macos.json")
