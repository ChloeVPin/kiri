#!/usr/bin/env python3
"""Repo-relative three-way comparison: startup markers, through-webview IPC, binary size.

Startup targets: kiri-host, wry-tao baseline, Tauri baseline.
IPC targets: kiri-host --ipc-bench vs Tauri kiri_echo (same payload sizes).
The in-process kiri-core bulk_bench is NOT run here and is not comparable to
Tauri invoke.

Only webview_ready-and-earlier startup phases are directly comparable across
all three targets (Q-003): Tauri routes later markers through invoke.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
COMPARABLE = ["platform_initialized", "webview_creation_requested", "webview_ready"]
FLAGGED = ["bridge_ready", "dom_ready", "app_ready", "first_animation_frame"]


def run(cmd, cwd=None, env=None, timeout=180):
    print("+", " ".join(str(c) for c in cmd), flush=True)
    return subprocess.run(
        cmd,
        cwd=cwd or ROOT,
        env=env,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout,
    )


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


def grab_file(path: Path):
    try:
        return grab_string(path.read_text())
    except OSError:
        return None


def median(xs):
    return statistics.median(xs) if xs else None


def percentile(xs, p):
    if not xs:
        return None
    values = sorted(xs)
    if len(values) == 1:
        return values[0]
    rank = (len(values) - 1) * p
    lower = int(rank)
    upper = min(lower + 1, len(values) - 1)
    if lower == upper:
        return values[lower]
    return values[lower] + (values[upper] - values[lower]) * (rank - lower)


def bin_size(path: Path):
    try:
        return path.stat().st_size
    except OSError:
        return None


def build(profile: str):
    flag = ["--release"] if profile == "release" else []
    run(["cargo", "build", "-p", "kiri-runtime", "--bin", "kiri-host", *flag], timeout=600)
    run(["cargo", "build", "--manifest-path", str(ROOT / "baselines/wry-tao/Cargo.toml"), *flag], timeout=600)
    run(["cargo", "build", "--manifest-path", str(ROOT / "baselines/tauri/Cargo.toml"), *flag], timeout=600)


def target_bins(profile: str):
    kind = "release" if profile == "release" else "debug"
    return {
        "kiri": ROOT / "target" / kind / "kiri-host",
        "wry": ROOT / "baselines/wry-tao/target" / kind / "wry-tao-baseline",
        "tauri": ROOT / "baselines/tauri/target" / kind / "tauri-baseline",
    }


def startup_once(label, cmd, out: Path, timeout):
    if out.exists():
        out.unlink()
    proc = subprocess.run(
        cmd,
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{label} startup failed rc={proc.returncode}\nstdout:\n{proc.stdout[-2000:]}\nstderr:\n{proc.stderr[-2000:]}"
        )
    data = grab_string(proc.stdout) or grab_file(out)
    if data is None:
        raise RuntimeError(f"{label}: no schema_version JSON\nstdout:\n{proc.stdout[-2000:]}")
    return {m["name"]: m["since_first_ns"] for m in data["markers"]}


def run_startup(bins, runs: int, timeout: int):
    artifacts = ROOT / "artifacts"
    artifacts.mkdir(exist_ok=True)
    specs = {
        "NATIVE disk (kiri-host)": {
            "cmd": [
                str(bins["kiri"]),
                "--smoke",
                "--frontend",
                str(ROOT / "examples/blank"),
                "--markers-out",
                str(artifacts / "startup-kiri-once.json"),
            ],
            "out": artifacts / "startup-kiri-once.json",
        },
        "NATIVE embedded": {
            "cmd": [
                str(bins["kiri"]),
                "--smoke",
                "--markers-out",
                str(artifacts / "startup-kiri-embedded-once.json"),
            ],
            "out": artifacts / "startup-kiri-embedded-once.json",
        },
        "WRY-TAO baseline": {
            "cmd": [str(bins["wry"])],
            "out": artifacts / "startup-wry-once.json",
        },
        "TAURI baseline": {
            "cmd": [str(bins["tauri"])],
            "out": artifacts / "startup-tauri-once.json",
        },
    }
    results = {}
    errors = {}
    for label, spec in specs.items():
        samples = []
        try:
            for i in range(runs):
                print(f"startup {label} run {i + 1}/{runs}", flush=True)
                samples.append(startup_once(label, spec["cmd"], spec["out"], timeout))
            results[label] = samples
        except Exception as exc:  # noqa: BLE001 — keep going so one failed target is recorded
            errors[label] = str(exc)
            results[label] = samples
    return results, errors


def run_ipc(bins, runs: int, timeout: int):
    artifacts = ROOT / "artifacts"
    artifacts.mkdir(exist_ok=True)
    kiri_out = artifacts / "ipc-kiri.json"
    tauri_out = artifacts / "ipc-tauri.json"
    for path in (kiri_out, tauri_out):
        if path.exists():
            path.unlink()

    kiri_cmd = [
        str(bins["kiri"]),
        "--frontend",
        str(ROOT / "examples/blank"),
        "--ipc-bench",
        "--ipc-bench-runs",
        str(runs),
        "--ipc-bench-out",
        str(kiri_out),
        "--markers-out",
        str(artifacts / "startup-kiri-ipc.json"),
    ]
    print("ipc kiri-host", flush=True)
    kiri_err = None
    try:
        run(kiri_cmd, timeout=timeout)
    except Exception as exc:  # noqa: BLE001
        kiri_err = str(exc)

    env = os.environ.copy()
    env["KIRI_IPC_BENCH"] = "1"
    env["KIRI_IPC_BENCH_RUNS"] = str(runs)
    env["KIRI_IPC_BENCH_OUT"] = str(tauri_out)
    print("ipc tauri-baseline", flush=True)
    tauri_err = None
    try:
        run([str(bins["tauri"])], env=env, timeout=timeout)
    except Exception as exc:  # noqa: BLE001
        tauri_err = str(exc)

    def load(path: Path, err):
        if path.exists():
            return json.loads(path.read_text())
        return {"error": err or f"missing {path}"}

    return {
        "kiri-host": load(kiri_out, kiri_err),
        "tauri-baseline": load(tauri_out, tauri_err),
    }


def print_startup(results, errors, runs):
    print(f"\nmacOS-native startup comparison (p50/p95/p99 of up to {runs} runs, ns since first marker)\n")
    labels = list(results)
    column_width = 34
    hdr = "marker".ljust(26) + "".join(l[:column_width].ljust(column_width) for l in labels)
    print(hdr)
    for name in COMPARABLE + FLAGGED:
        row = name.ljust(26)
        for label in labels:
            samples = results.get(label) or []
            vals = [s.get(name) for s in samples if s.get(name) is not None]
            if not vals:
                row += "n/a".ljust(column_width)
            else:
                row += (
                    f"{int(median(vals)):,}/"
                    f"{int(percentile(vals, 0.95)):,}/"
                    f"{int(percentile(vals, 0.99)):,}"
                ).ljust(column_width)
        tag = ""
        if name in COMPARABLE:
            tag = "  [comparable]"
        elif name in ("dom_ready", "app_ready", "first_animation_frame"):
            tag = "  [TAURI non-comparable: invoke path]"
        print(row + tag)
    if errors:
        print("\nstartup errors:")
        for label, err in errors.items():
            print(f"  {label}: {err[:400]}")


def print_ipc(ipc):
    print("\nthrough-webview IPC (page → host → page). Not in-process bulk_bench.\n")
    print(
        "size".ljust(14)
        + "kiri batch-mean".ljust(18)
        + "tauri batch-mean".ljust(18)
        + "kiri/tauri"
    )
    kiri = {r.get("size_bytes"): r for r in ipc.get("kiri-host", {}).get("results", []) or []}
    tauri = {r.get("size_bytes"): r for r in ipc.get("tauri-baseline", {}).get("results", []) or []}
    sizes = sorted(set(kiri) | set(tauri))
    if not sizes:
        print("  no IPC samples")
        if ipc.get("kiri-host", {}).get("error"):
            print("  kiri:", ipc["kiri-host"]["error"])
        if ipc.get("tauri-baseline", {}).get("error"):
            print("  tauri:", ipc["tauri-baseline"]["error"])
        return

    def batch_mean(entry):
        if not entry:
            return None
        summary = entry.get("summary") or {}
        return (
            summary.get("mean_from_batch_ms")
            or entry.get("mean_from_batch_ms")
            or summary.get("median_ms")
        )

    def rtt_percentiles(entry):
        if not entry:
            return None, None
        values = [v for v in entry.get("rtt_ms", []) if isinstance(v, (int, float))]
        return percentile(values, 0.95), percentile(values, 0.99)

    for size in sizes:
        km = batch_mean(kiri.get(size))
        tm = batch_mean(tauri.get(size))
        ratio = f"{km / tm:.2f}" if km and tm else "n/a"
        ktxt = f"{km:.3f}" if isinstance(km, (int, float)) else "n/a"
        ttxt = f"{tm:.3f}" if isinstance(tm, (int, float)) else "n/a"
        print(f"{size}".ljust(14) + ktxt.ljust(18) + ttxt.ljust(18) + ratio)
        kp95, kp99 = rtt_percentiles(kiri.get(size))
        tp95, tp99 = rtt_percentiles(tauri.get(size))
        k_tail = f"{kp95:.3f}/{kp99:.3f}" if kp95 is not None else "n/a"
        t_tail = f"{tp95:.3f}/{tp99:.3f}" if tp95 is not None else "n/a"
        print("  rtt p95/p99".ljust(14) + k_tail.ljust(18) + t_tail)
    print("\nPer-call performance.now() samples are often 0 or 1 ms on WKWebView;")
    print("batch-mean (total batch time / N) is the comparable figure.")


def print_sizes(bins):
    print("\nrelease/debug binary sizes (bytes, unstripped unless the profile strips)\n")
    for name, path in bins.items():
        print(f"  {name:8} {bin_size(path) or 'missing':>12}  {path}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", choices=("debug", "release"), default="release")
    parser.add_argument("--startup-runs", type=int, default=5)
    parser.add_argument("--ipc-runs", type=int, default=30)
    parser.add_argument("--skip-build", action="store_true")
    parser.add_argument("--startup-timeout", type=int, default=60)
    parser.add_argument("--ipc-timeout", type=int, default=180)
    args = parser.parse_args()

    if not args.skip_build:
        build(args.profile)
    bins = target_bins(args.profile)
    startup, startup_errors = run_startup(bins, args.startup_runs, args.startup_timeout)
    ipc = run_ipc(bins, args.ipc_runs, args.ipc_timeout)

    print_startup(startup, startup_errors, args.startup_runs)
    print_ipc(ipc)
    print_sizes(bins)

    artifact = {
        "schema_version": 1,
        "name": "kiri-vs-tauri-compare",
        "platform": f"{platform.system()} {platform.machine()}",
        "processor": platform.processor(),
        "profile": args.profile,
        "startup_runs": args.startup_runs,
        "ipc_runs": args.ipc_runs,
        "startup": startup,
        "startup_errors": startup_errors,
        "ipc": ipc,
        "binary_bytes": {k: bin_size(v) for k, v in bins.items()},
    }
    out = ROOT / "artifacts" / "compare-macos.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps(artifact, indent=2))
    print(f"\nraw artifact -> {out}")
    if startup_errors or any(v.get("error") for v in ipc.values() if isinstance(v, dict)):
        sys.exit(1)


if __name__ == "__main__":
    main()
