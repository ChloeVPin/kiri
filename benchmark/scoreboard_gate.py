#!/usr/bin/env python3
"""Scoreboard proof gate for through-webview IPC artifacts.

Validates that an artifact intended for the public scoreboard
(docs/COMPETITIVE_ANALYSIS.md) carries enough evidence to publish an honest
chart, and REFUSES anything that cannot prove its claim:

- metric class must be `through-webview-ipc` (registered classes only).
  In-process microbenches such as `bulk_bench` / `ordinary-message-bulk-path`
  are refused outright: they are not what an application feels.
- required provenance: run id or local host id, runner/OS, commit, payload
  sizes, batch-mean and/or per-call distribution, iteration count, warmups.
- any `shared-buffer` claim must carry per-size proof counts
  (`shared_buffer_used` / `shared_buffer_hits`) plus top-level reply and
  fallback counts (`shared_buffer.replies_ok` / `replies_fallback`).
- any `ring-zerocopy` claim must declare `transport: "ring_zerocopy"`,
  carry a top-level `ring` block (`replies_ok` / `replies_fallback` /
  `send_fallbacks`), and record per-size `ring_slot_hits` on every result.
- `zero-copy` is a dual-path claim: it is proved by the ring contract when
  the artifact ran the ring transport or recorded ring traffic, and by the
  shared-buffer contract otherwise. Zero proof on both paths is refused. A
  claim without proof is refused; a losing measurement with honest numbers
  is accepted.
- artifacts marked `fixture`/`example` are refused by default so example data
  can never reach a published table (`--allow-fixtures` exists for tests).

A successor mechanism plugs in by registering its metric class in
ACCEPTED_METRIC_CLASSES and its proof checker in PROOF_CHECKERS in this
file, then emitting the same provenance block plus its own per-size proof
counts; the zero-copy ring transport (`transport: "ring_zerocopy"`) is
registered this way. Until a claim has a registered proof contract,
artifacts asserting it are refused.

Usage:
    scoreboard_gate.py check FILE... [--allow-fixtures] [--verdict-out PATH]
    scoreboard_gate.py stamp FILE [--out PATH] [--run-id ID] [--runner NAME]
                           [--os OS] [--arch ARCH] [--host-id ID]
                           [--commit SHA] [--claim CLAIM]...

`check` exits 0 when every artifact is accepted, 1 when any is refused.
`stamp` injects publish provenance (run/runner/host/commit/claims) into an
existing artifact so locally produced files can satisfy the contract without
editing measured numbers.
"""
from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# Metric classes allowed on the public scoreboard. `bulk_bench` and any other
# in-process router microbench are deliberately absent: they never touch a
# WebView and must never be dressed up as IPC an application feels.
ACCEPTED_METRIC_CLASSES = {"through-webview-ipc"}
BANNED_CLASS_MARKERS = ("bulk", "in-process", "in_process", "router")

# Claim names an artifact may assert. Aliases normalize to a canonical claim.
CLAIM_ALIASES = {
    "shared-buffer": "shared-buffer",
    "shared_buffer": "shared-buffer",
    "sharedbuffer": "shared-buffer",
    "zero-copy": "zero-copy",
    "zerocopy": "zero-copy",
    "zero_copy": "zero-copy",
    "ring-zerocopy": "ring-zerocopy",
    "ring_zerocopy": "ring-zerocopy",
    "ringzerocopy": "ring-zerocopy",
    "ring": "ring-zerocopy",
    "protocol-ring": "protocol-ring",
    "protocol_ring": "protocol-ring",
    "protoring": "protocol-ring",
    "proto-ring": "protocol-ring",
    "proto_ring": "protocol-ring",
}


def _is_int(value) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _is_number(value) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _nonempty_str(value) -> bool:
    return isinstance(value, str) and bool(value.strip())


def _shared_buffer_proof_reasons(artifact: dict, claim: str) -> list[str]:
    """Proof contract for the WebView2 shared-buffer path (and, until a
    successor registers its own fields, for `zero-copy` claims).

    Required: per-size `shared_buffer_used` (bool) and `shared_buffer_hits`
    (int) on every result, plus top-level `shared_buffer.replies_ok` and
    `shared_buffer.replies_fallback` counts. Fallbacks are legal but must be
    reported; zero observed shared-buffer replies cannot prove the claim.
    """
    reasons: list[str] = []
    block = artifact.get("shared_buffer")
    if not isinstance(block, dict):
        reasons.append(
            f"claim '{claim}' requires a top-level 'shared_buffer' proof block "
            "with replies_ok and replies_fallback counts"
        )
    else:
        for key in ("replies_ok", "replies_fallback"):
            if not _is_int(block.get(key)) or block.get(key, -1) < 0:
                reasons.append(
                    f"claim '{claim}' requires shared_buffer.{key} as a "
                    "non-negative integer (fallback counts must be reported)"
                )
    results = artifact.get("results") or []
    for i, entry in enumerate(results):
        size = entry.get("size_bytes", "?")
        if not isinstance(entry.get("shared_buffer_used"), bool):
            reasons.append(
                f"claim '{claim}' requires results[{i}] (size_bytes={size}) "
                "to record shared_buffer_used"
            )
        if not _is_int(entry.get("shared_buffer_hits")):
            reasons.append(
                f"claim '{claim}' requires results[{i}] (size_bytes={size}) "
                "to record shared_buffer_hits per size"
            )
    if not reasons and isinstance(block, dict):
        hits = sum(e.get("shared_buffer_hits") or 0 for e in results)
        used = any(e.get("shared_buffer_used") for e in results)
        if block.get("replies_ok", 0) == 0 and hits == 0 and not used:
            reasons.append(
                f"claim '{claim}' recorded zero shared-buffer replies: "
                "no proof the path was exercised"
            )
    return reasons


def _has_ring_traffic(artifact: dict) -> bool:
    """True when the artifact recorded actual ring replies or slot hits."""
    block = artifact.get("ring")
    if (
        isinstance(block, dict)
        and _is_int(block.get("replies_ok"))
        and block["replies_ok"] > 0
    ):
        return True
    return any(
        isinstance(entry, dict)
        and _is_int(entry.get("ring_slot_hits"))
        and entry["ring_slot_hits"] > 0
        for entry in artifact.get("results") or []
    )


def _ring_zerocopy_proof_reasons(artifact: dict, claim: str) -> list[str]:
    """Proof contract for the zero-copy ring transport
    (`transport: "ring_zerocopy"`).

    Required: the artifact must declare the ring transport, carry a top-level
    `ring` block with `replies_ok` / `replies_fallback` / `send_fallbacks`
    counts, and record `ring_slot_hits` on every result. Fallbacks are legal
    but must be reported; zero observed ring replies cannot prove the claim.
    """
    reasons: list[str] = []
    transport = artifact.get("transport")
    if transport != "ring_zerocopy":
        reasons.append(
            f"claim '{claim}' requires transport 'ring_zerocopy' "
            f"(got {transport!r}); an artifact that did not run the ring "
            "transport cannot prove a ring claim"
        )
    block = artifact.get("ring")
    if not isinstance(block, dict):
        reasons.append(
            f"claim '{claim}' requires a top-level 'ring' proof block "
            "with replies_ok, replies_fallback, and send_fallbacks counts"
        )
    else:
        for key in ("replies_ok", "replies_fallback", "send_fallbacks"):
            if not _is_int(block.get(key)) or block.get(key, -1) < 0:
                reasons.append(
                    f"claim '{claim}' requires ring.{key} as a "
                    "non-negative integer (fallback counts must be reported)"
                )
    results = artifact.get("results") or []
    for i, entry in enumerate(results):
        if not isinstance(entry, dict):
            continue
        size = entry.get("size_bytes", "?")
        if not _is_int(entry.get("ring_slot_hits")) or entry.get("ring_slot_hits", -1) < 0:
            reasons.append(
                f"claim '{claim}' requires results[{i}] (size_bytes={size}) "
                "to record ring_slot_hits per size"
            )
    if not reasons and isinstance(block, dict):
        hits = sum(
            entry.get("ring_slot_hits") or 0 for entry in results if isinstance(entry, dict)
        )
        if block.get("replies_ok", 0) == 0 and hits == 0:
            reasons.append(
                f"claim '{claim}' recorded zero ring replies: "
                "no proof the ring transport was exercised"
            )
    return reasons


def _protocol_ring_proof_reasons(artifact: dict, claim: str) -> list[str]:
    """Proof contract for the protocol-ring transport
    (`transport: "protocol_ring"`).

    The wry/WKWebView spike path: one binary KRSL frame per direction over
    the `kiri://` app scheme, dispatched through the same ZcIpcGate as the
    postMessage pipe. Required: the artifact must declare the protocol_ring
    transport, carry a top-level `protocol_ring` block with
    `replies_ok` / `replies_fallback` / `send_fallbacks` counts, and record
    `proto_ring_hits` on every result. Fallbacks are legal but must be
    reported; zero observed binary replies cannot prove the claim.
    """
    reasons: list[str] = []
    transport = artifact.get("transport")
    if transport != "protocol_ring":
        reasons.append(
            f"claim '{claim}' requires transport 'protocol_ring' "
            f"(got {transport!r}); an artifact that did not run the "
            "protocol_ring transport cannot prove a protocol-ring claim"
        )
    block = artifact.get("protocol_ring")
    if not isinstance(block, dict):
        reasons.append(
            f"claim '{claim}' requires a top-level 'protocol_ring' proof "
            "block with replies_ok, replies_fallback, and send_fallbacks counts"
        )
    else:
        for key in ("replies_ok", "replies_fallback", "send_fallbacks"):
            if not _is_int(block.get(key)) or block.get(key, -1) < 0:
                reasons.append(
                    f"claim '{claim}' requires protocol_ring.{key} as a "
                    "non-negative integer (fallback counts must be reported)"
                )
    results = artifact.get("results") or []
    for i, entry in enumerate(results):
        if not isinstance(entry, dict):
            continue
        size = entry.get("size_bytes", "?")
        if not _is_int(entry.get("proto_ring_hits")) or entry.get("proto_ring_hits", -1) < 0:
            reasons.append(
                f"claim '{claim}' requires results[{i}] (size_bytes={size}) "
                "to record proto_ring_hits per size"
            )
    if not reasons and isinstance(block, dict):
        hits = sum(
            entry.get("proto_ring_hits") or 0 for entry in results if isinstance(entry, dict)
        )
        if block.get("replies_ok", 0) == 0 and hits == 0:
            reasons.append(
                f"claim '{claim}' recorded zero binary replies: "
                "no proof the protocol_ring transport was exercised"
            )
    return reasons


def _zero_copy_proof_reasons(artifact: dict, claim: str) -> list[str]:
    """`zero-copy` is dual-path: the ring contract proves it when the
    artifact ran `transport: "ring_zerocopy"` or recorded ring traffic,
    otherwise the WebView2 shared-buffer contract (T008) applies. An
    artifact with zero shared-buffer and zero ring evidence is refused by
    whichever checker owns it."""
    if artifact.get("transport") == "ring_zerocopy" or _has_ring_traffic(artifact):
        return _ring_zerocopy_proof_reasons(artifact, claim)
    return _shared_buffer_proof_reasons(artifact, claim)


# Proof checkers keyed by canonical claim. `ring-zerocopy` names the
# zero-copy ring transport (one host-owned slot arena, raw payload bytes,
# tiny JSON control messages). `zero-copy` dispatches between the ring and
# shared-buffer contracts as described above; a successor mechanism must
# register its own checker (and proof fields) here before its artifacts can
# pass.
PROOF_CHECKERS = {
    "shared-buffer": _shared_buffer_proof_reasons,
    "ring-zerocopy": _ring_zerocopy_proof_reasons,
    "zero-copy": _zero_copy_proof_reasons,
    "protocol-ring": _protocol_ring_proof_reasons,
}


def _run_block(artifact: dict) -> dict:
    run = artifact.get("run")
    return run if isinstance(run, dict) else {}


def _provenance(artifact: dict) -> tuple[object, object]:
    run = _run_block(artifact)
    env = artifact.get("environment")
    env = env if isinstance(env, dict) else {}
    run_id = (
        run.get("id")
        or artifact.get("run_id")
        or run.get("host_id")
        or artifact.get("host_id")
    )
    runner = (
        run.get("os")
        or run.get("runner")
        or artifact.get("os")
        or artifact.get("runner")
        or env.get("platform")
    )
    return run_id, runner


def _claims(artifact: dict) -> list[str]:
    """Explicit claims plus implicit ones: an artifact that recorded shared
    buffer traffic asserts the shared-buffer claim, and an artifact that ran
    the ring transport or recorded ring traffic asserts the ring-zerocopy
    claim, even without a claims array, so their proof is checked too."""
    found: list[str] = []
    raw = artifact.get("claims")
    if isinstance(raw, list):
        for item in raw:
            if isinstance(item, str):
                found.append(CLAIM_ALIASES.get(item.strip().lower(), item.strip().lower()))
    block = artifact.get("shared_buffer")
    implicit = isinstance(block, dict) and (block.get("replies_ok") or 0) > 0
    implicit_ring = artifact.get("transport") == "ring_zerocopy"
    ring = artifact.get("ring")
    if isinstance(ring, dict) and _is_int(ring.get("replies_ok")) and ring["replies_ok"] > 0:
        implicit_ring = True
    # A protocol_ring artifact asserts the claim only on positive binary
    # traffic: a run that fell back entirely is honest data, not a claim.
    implicit_proto = False
    proto = artifact.get("protocol_ring")
    if isinstance(proto, dict) and _is_int(proto.get("replies_ok")) and proto["replies_ok"] > 0:
        implicit_proto = True
    for entry in artifact.get("results") or []:
        if not isinstance(entry, dict):
            continue
        if entry.get("shared_buffer_used") or (entry.get("shared_buffer_hits") or 0) > 0:
            implicit = True
        if _is_int(entry.get("ring_slot_hits")) and entry["ring_slot_hits"] > 0:
            implicit_ring = True
        if _is_int(entry.get("proto_ring_hits")) and entry["proto_ring_hits"] > 0:
            implicit_proto = True
    if implicit and "shared-buffer" not in found:
        found.append("shared-buffer")
    if implicit_ring and "ring-zerocopy" not in found:
        found.append("ring-zerocopy")
    if implicit_proto and "protocol-ring" not in found:
        found.append("protocol-ring")
    return found


def validate(path: Path, artifact, allow_fixtures: bool = False) -> tuple[list[str], list[str]]:
    """Return (refusal_reasons, warnings). Any reason means REFUSED."""
    reasons: list[str] = []
    warnings: list[str] = []

    if not isinstance(artifact, dict):
        return ["artifact is not a JSON object"], warnings

    if artifact.get("fixture") is True or artifact.get("example") is True:
        if allow_fixtures:
            warnings.append("fixture/example data; not publishable evidence")
        else:
            reasons.append(
                "artifact is labeled fixture/example data and is not publishable "
                "evidence (pass --allow-fixtures to validate shape only)"
            )

    error = artifact.get("error")
    if error not in (None, "", False):
        reasons.append(f"artifact records a bench error: {error}")

    metric = artifact.get("metric_class") or artifact.get("name")
    if not _nonempty_str(metric):
        reasons.append("missing metric class ('name' or 'metric_class')")
    else:
        lowered = metric.strip().lower()
        if any(marker in lowered for marker in BANNED_CLASS_MARKERS):
            reasons.append(
                f"metric class '{metric}' is an in-process/bulk microbench, "
                "not through-webview IPC; it cannot be published as an IPC result"
            )
        elif metric not in ACCEPTED_METRIC_CLASSES:
            reasons.append(
                f"metric class '{metric}' is not registered for the scoreboard "
                f"(accepted: {', '.join(sorted(ACCEPTED_METRIC_CLASSES))}); "
                "register the class and its proof contract in scoreboard_gate.py"
            )

    if not _nonempty_str(artifact.get("target")):
        reasons.append("missing 'target' (which host produced these numbers)")

    commit = artifact.get("commit")
    if not _nonempty_str(commit) or commit.strip() == "unknown":
        reasons.append("missing 'commit'; 'unknown' is not publishable provenance")

    run_id, runner = _provenance(artifact)
    if not _nonempty_str(run_id) and not _is_int(run_id):
        reasons.append(
            "missing run identity: publish a run.id / run_id (hosted) or "
            "run.host_id / host_id (local machine identity)"
        )
    if not _nonempty_str(runner):
        reasons.append(
            "missing runner/OS metadata (run.os, run.runner, os, or "
            "environment.platform); hosted numbers need their runner label"
        )

    if not _is_int(artifact.get("runs")) or artifact.get("runs", 0) <= 0:
        reasons.append("missing iteration count ('runs' must be a positive integer)")
    if not _is_int(artifact.get("warmup")) or artifact.get("warmup", -1) < 0:
        reasons.append("missing warmup count ('warmup' must be a non-negative integer)")

    results = artifact.get("results")
    if not isinstance(results, list) or not results:
        reasons.append("missing 'results' array with per-size measurements")
        results = []
    sizes = artifact.get("sizes_bytes")
    if not isinstance(sizes, list) or not sizes:
        sizes = [e.get("size_bytes") for e in results if _is_int(e.get("size_bytes"))]
    if not sizes:
        reasons.append("missing payload sizes ('sizes_bytes' or per-result 'size_bytes')")

    for i, entry in enumerate(results):
        if not isinstance(entry, dict):
            reasons.append(f"results[{i}] is not an object")
            continue
        if not _is_int(entry.get("size_bytes")) or entry.get("size_bytes", -1) < 0:
            reasons.append(f"results[{i}] missing size_bytes")
        summary = entry.get("summary") if isinstance(entry.get("summary"), dict) else {}
        batch_mean = entry.get("mean_from_batch_ms") or summary.get("mean_from_batch_ms")
        rtt = entry.get("rtt_ms")
        distribution = (
            isinstance(rtt, list)
            and bool(rtt)
            and all(_is_number(v) for v in rtt)
        )
        if not _is_number(batch_mean) and not distribution:
            reasons.append(
                f"results[{i}] needs batch-mean ('mean_from_batch_ms') and/or "
                "a numeric per-call distribution ('rtt_ms')"
            )

    for claim in _claims(artifact):
        checker = PROOF_CHECKERS.get(claim)
        if checker is None:
            reasons.append(
                f"claim '{claim}' has no registered proof contract; refusing "
                "to publish an unprovable claim"
            )
        else:
            reasons.extend(checker(artifact, claim))

    block = artifact.get("shared_buffer")
    if isinstance(block, dict) and (block.get("replies_fallback") or 0) > 0:
        warnings.append(
            f"{block['replies_fallback']} JSON fallback replies observed; "
            "publish fallback counts alongside shared-buffer counts"
        )

    ring = artifact.get("ring")
    if isinstance(ring, dict):
        ring_fallbacks = sum(
            ring[key] for key in ("replies_fallback", "send_fallbacks") if _is_int(ring.get(key))
        )
        if ring_fallbacks > 0:
            warnings.append(
                f"{ring_fallbacks} ring fallback sends/replies observed; "
                "publish fallback counts alongside ring counts"
            )

    return reasons, warnings


def check(paths: list[Path], allow_fixtures: bool, verdict_out: Path | None) -> int:
    verdicts = []
    for path in paths:
        try:
            artifact = json.loads(path.read_text(encoding="utf-8"))
        except FileNotFoundError:
            verdicts.append(
                {"path": str(path), "status": "refused",
                 "reasons": ["artifact file does not exist"], "warnings": []}
            )
            continue
        except (OSError, json.JSONDecodeError) as exc:
            verdicts.append(
                {"path": str(path), "status": "refused",
                 "reasons": [f"artifact is not readable JSON: {exc}"], "warnings": []}
            )
            continue
        reasons, warnings = validate(path, artifact, allow_fixtures)
        status = "refused" if reasons else "accepted"
        entry = {
            "path": str(path),
            "status": status,
            "reasons": reasons,
            "warnings": warnings,
        }
        if isinstance(artifact, dict):
            entry["metric_class"] = artifact.get("metric_class") or artifact.get("name")
            entry["target"] = artifact.get("target")
        verdicts.append(entry)

    for entry in verdicts:
        if entry["status"] == "accepted":
            tag = f" ({entry.get('metric_class')} target={entry.get('target')})"
            print(f"ACCEPTED {entry['path']}{tag}")
        else:
            print(f"REFUSED {entry['path']}:")
            for reason in entry["reasons"]:
                print(f"  - {reason}")
        for warning in entry["warnings"]:
            print(f"  warning: {warning}")

    ok = all(v["status"] == "accepted" for v in verdicts)
    verdict = {
        "schema_version": 1,
        "name": "scoreboard-gate-verdict",
        "status": "accepted" if ok else "refused",
        "artifacts": verdicts,
    }
    if verdict_out is not None:
        verdict_out.parent.mkdir(parents=True, exist_ok=True)
        verdict_out.write_text(json.dumps(verdict, indent=2) + "\n", encoding="utf-8")
        print(f"verdict -> {verdict_out}")
    if not ok:
        print("gate: REFUSED. Fix the artifact or do not publish the claim.")
    return 0 if ok else 1


def _git_commit() -> str | None:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        return out.stdout.strip() or None
    except Exception:  # noqa: BLE001
        return None


def stamp(args) -> int:
    path = Path(args.file)
    artifact = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(artifact, dict):
        print("cannot stamp: artifact is not a JSON object", file=sys.stderr)
        return 1

    run = artifact.get("run")
    if not isinstance(run, dict):
        run = {}
        artifact["run"] = run

    run_id = args.run_id or os.environ.get("GITHUB_RUN_ID")
    if run_id:
        run["id"] = run_id
        server = os.environ.get("GITHUB_SERVER_URL", "https://github.com")
        repo = os.environ.get("GITHUB_REPOSITORY")
        if repo:
            run["url"] = f"{server}/{repo}/actions/runs/{run_id}"
    run["runner"] = args.runner or os.environ.get("RUNNER_NAME") or "local"
    run["os"] = args.os or os.environ.get("RUNNER_OS") or platform.system()
    run["arch"] = args.arch or os.environ.get("RUNNER_ARCH") or platform.machine()
    host_id = (
        args.host_id
        or os.environ.get("KIRI_BENCH_HOST_ID")
        or platform.node()
        or os.environ.get("HOSTNAME")
        or os.environ.get("COMPUTERNAME")
    )
    if host_id:
        run["host_id"] = host_id
    commit = args.commit or _git_commit()
    if commit:
        artifact["commit"] = commit
    if args.claim:
        existing = artifact.get("claims")
        merged = sorted({*(existing if isinstance(existing, list) else []), *args.claim})
        artifact["claims"] = merged

    text = json.dumps(artifact, indent=2) + "\n"
    if args.out:
        out = Path(args.out)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(text, encoding="utf-8")
        print(f"stamped -> {out}")
    else:
        sys.stdout.write(text)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    check_p = sub.add_parser("check", help="validate artifacts against the scoreboard contract")
    check_p.add_argument("files", nargs="+", type=Path)
    check_p.add_argument("--allow-fixtures", action="store_true",
                         help="validate fixture/example artifacts by shape (tests only)")
    check_p.add_argument("--verdict-out", type=Path, default=None,
                         help="write the verdict JSON (status: accepted|refused)")

    stamp_p = sub.add_parser("stamp", help="inject publish provenance into an artifact")
    stamp_p.add_argument("file")
    stamp_p.add_argument("--out", default=None, help="output path (default: stdout)")
    stamp_p.add_argument("--run-id", default=None)
    stamp_p.add_argument("--runner", default=None)
    stamp_p.add_argument("--os", default=None)
    stamp_p.add_argument("--arch", default=None)
    stamp_p.add_argument("--host-id", default=None)
    stamp_p.add_argument("--commit", default=None)
    stamp_p.add_argument("--claim", action="append", default=[],
                         help="assert a claim (repeatable); claims require proof at check time")

    args = parser.parse_args()
    if args.command == "check":
        return check(args.files, args.allow_fixtures, args.verdict_out)
    return stamp(args)


if __name__ == "__main__":
    raise SystemExit(main())
