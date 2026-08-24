#!/usr/bin/env python3
import argparse, json, os, platform, statistics, subprocess, time
from pathlib import Path

try:
    import resource
except ImportError:  # Windows does not expose POSIX child accounting.
    resource = None


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
    parser.add_argument('--timeout-seconds', type=float, default=60.0)
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
        'child_resource_accounting': 'posix-resource' if resource is not None else 'unavailable',
    }

    def child_usage():
        if resource is None:
            return None
        usage = resource.getrusage(resource.RUSAGE_CHILDREN)
        return {
            'user_ms': usage.ru_utime * 1000.0,
            'system_ms': usage.ru_stime * 1000.0,
            # ru_maxrss is bytes on macOS and KiB on Linux/BSD.
            'max_rss_bytes': int(usage.ru_maxrss if platform.system() == 'Darwin' else usage.ru_maxrss * 1024),
        }

    def usage_delta(before, after):
        if before is None or after is None:
            return {}
        return {
            'child_user_ms': max(0.0, after['user_ms'] - before['user_ms']),
            'child_system_ms': max(0.0, after['system_ms'] - before['system_ms']),
            # ru_maxrss is the maximum over all waited-for children, so it is
            # retained as an observed high-water mark, not a per-run delta.
            'child_max_rss_bytes_high_water': after['max_rss_bytes'],
        }

    for _ in range(args.warmups):
        subprocess.run(
            command,
            check=True,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=args.timeout_seconds,
        )

    samples_ms = []
    runs = []
    for i in range(args.runs):
        t0 = time.perf_counter_ns()
        usage_before = child_usage()
        try:
            proc = subprocess.run(
                command,
                capture_output=True,
                text=True,
                timeout=args.timeout_seconds,
            )
        except subprocess.TimeoutExpired as exc:
            elapsed_ms = (time.perf_counter_ns() - t0) / 1_000_000
            runs.append({
                'index': i,
                'elapsed_ms': elapsed_ms,
                'returncode': 124,
                'stdout': (exc.stdout or '')[-4096:],
                'stderr': (
                    (exc.stderr or '') +
                    f'benchmark command timed out after {args.timeout_seconds:g}s'
                )[-4096:],
                **usage_delta(usage_before, child_usage()),
            })
            samples_ms.append(elapsed_ms)
            break
        t1 = time.perf_counter_ns()
        elapsed_ms = (t1 - t0) / 1_000_000
        usage_after = child_usage()
        samples_ms.append(elapsed_ms)
        runs.append({
            'index': i,
            'elapsed_ms': elapsed_ms,
            'returncode': proc.returncode,
            'stdout': proc.stdout[-4096:],
            'stderr': proc.stderr[-4096:],
            **usage_delta(usage_before, usage_after),
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
    resource_runs = [run for run in runs if 'child_user_ms' in run]
    if resource_runs:
        result['summary'].update({
            'mean_child_user_ms': statistics.fmean(r['child_user_ms'] for r in resource_runs),
            'mean_child_system_ms': statistics.fmean(r['child_system_ms'] for r in resource_runs),
            'child_max_rss_bytes_high_water': max(
                r['child_max_rss_bytes_high_water'] for r in resource_runs
            ),
        })
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps(result['summary'], indent=2))
    if any(run['returncode'] != 0 for run in runs):
        raise SystemExit(1)


if __name__ == '__main__':
    main()
