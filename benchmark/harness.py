#!/usr/bin/env python3
import argparse, json, os, platform, statistics, subprocess, time
from pathlib import Path


def percentile(values, p):
    if not values:
        return None
    xs = sorted(values)
    if len(xs) == 1:
        return xs[0]
    k = (len(xs) - 1) * p
    f = int(k)
    c = min(f + 1, len(xs) - 1)
    if f == c:
        return xs[f]
    return xs[f] + (xs[c] - xs[f]) * (k - f)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--name', required=True)
    parser.add_argument('--runs', type=int, default=20)
    parser.add_argument('--warmups', type=int, default=3)
    parser.add_argument('--output', required=True)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command and command[0] == '--':
        command = command[1:]
    if not command:
        raise SystemExit('missing command after --')

    env = {
        'platform': platform.platform(),
        'machine': platform.machine(),
        'processor': platform.processor(),
        'python': platform.python_version(),
        'cpu_count': os.cpu_count(),
    }

    for _ in range(args.warmups):
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

    samples_ms = []
    runs = []
    for i in range(args.runs):
        t0 = time.perf_counter_ns()
        proc = subprocess.run(command, capture_output=True, text=True)
        t1 = time.perf_counter_ns()
        elapsed_ms = (t1 - t0) / 1_000_000
        samples_ms.append(elapsed_ms)
        runs.append({
            'index': i,
            'elapsed_ms': elapsed_ms,
            'returncode': proc.returncode,
            'stdout': proc.stdout[-4096:],
            'stderr': proc.stderr[-4096:],
        })
        if proc.returncode != 0:
            break

    result = {
        'schema_version': 1,
        'name': args.name,
        'command': command,
        'created_unix_ns': time.time_ns(),
        'environment': env,
        'samples_ms': samples_ms,
        'summary': {
            'count': len(samples_ms),
            'mean_ms': statistics.fmean(samples_ms) if samples_ms else None,
            'median_ms': statistics.median(samples_ms) if samples_ms else None,
            'p95_ms': percentile(samples_ms, 0.95),
            'p99_ms': percentile(samples_ms, 0.99),
            'min_ms': min(samples_ms) if samples_ms else None,
            'max_ms': max(samples_ms) if samples_ms else None,
        },
        'runs': runs,
    }
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result['summary'], indent=2))


if __name__ == '__main__':
    main()
