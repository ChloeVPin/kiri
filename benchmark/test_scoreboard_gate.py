import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "benchmark" / "scoreboard_gate.py"
FIXTURES = ROOT / "benchmark" / "fixtures"


def run_gate(*args, env_extra=None):
    import os

    env = os.environ.copy()
    env.update(env_extra or {})
    return subprocess.run(
        [sys.executable, str(GATE), *args],
        cwd=ROOT,
        capture_output=True,
        text=True,
        env=env,
    )


def valid_artifact():
    return {
        "schema_version": 1,
        "name": "through-webview-ipc",
        "target": "kiri-host",
        "commit": "abc1234",
        "runs": 5,
        "warmup": 2,
        "sizes_bytes": [64, 262144],
        "run": {
            "id": "123456",
            "runner": "hosted-runner",
            "os": "macOS",
            "arch": "arm64",
        },
        "shared_buffer": {
            "threshold_bytes": 65536,
            "replies_ok": 0,
            "replies_fallback": 0,
        },
        "results": [
            {
                "size_bytes": 64,
                "rtt_ms": [0.1, 0.1, 0.2, 0.1, 0.1],
                "batch_ms": 0.6,
                "mean_from_batch_ms": 0.12,
                "shared_buffer_hits": 0,
                "shared_buffer_used": False,
            },
            {
                "size_bytes": 262144,
                "rtt_ms": [4.5, 4.6, 4.7, 4.8, 4.9],
                "batch_ms": 23.5,
                "mean_from_batch_ms": 4.7,
                "shared_buffer_hits": 0,
                "shared_buffer_used": False,
            },
        ],
    }


def ring_artifact():
    artifact = valid_artifact()
    artifact["transport"] = "ring_zerocopy"
    artifact["ring"] = {
        "replies_ok": 10,
        "replies_fallback": 0,
        "send_fallbacks": 0,
    }
    for entry in artifact["results"]:
        entry["ring_slot_hits"] = 5
        entry["ring_send_fallbacks"] = 0
    return artifact


def write_tmp(directory, artifact):
    path = Path(directory) / "artifact.json"
    path.write_text(json.dumps(artifact), encoding="utf-8")
    return path


class ScoreboardGateTests(unittest.TestCase):
    def test_shared_buffer_fixture_is_accepted_with_allow_fixtures(self):
        completed = run_gate(
            "check",
            "--allow-fixtures",
            str(FIXTURES / "through-webview-ipc.shared-buffer.example.json"),
        )
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        self.assertIn("ACCEPTED", completed.stdout)

    def test_fixtures_are_refused_by_default(self):
        completed = run_gate(
            "check", str(FIXTURES / "through-webview-ipc.shared-buffer.example.json")
        )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("REFUSED", completed.stdout)
        self.assertIn("fixture", completed.stdout.lower())

    def test_bulk_bench_shaped_artifact_is_refused(self):
        completed = run_gate(
            "check",
            "--allow-fixtures",
            str(FIXTURES / "ordinary-message-bulk-path.example.json"),
        )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("REFUSED", completed.stdout)
        self.assertIn("in-process", completed.stdout)

    def test_shared_buffer_claim_without_proof_is_refused(self):
        completed = run_gate(
            "check",
            "--allow-fixtures",
            str(FIXTURES / "through-webview-ipc.claim-no-proof.example.json"),
        )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("REFUSED", completed.stdout)
        self.assertIn("shared_buffer", completed.stdout)

    def test_valid_artifact_is_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, valid_artifact())
            completed = run_gate("check", str(path))
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
            self.assertIn("ACCEPTED", completed.stdout)

    def test_missing_run_identity_is_refused(self):
        artifact = valid_artifact()
        artifact["run"] = {"os": "macOS", "runner": "hosted-runner"}
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("run identity", completed.stdout)

    def test_missing_runner_os_is_refused(self):
        artifact = valid_artifact()
        artifact["run"] = {"id": "123"}
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("runner/OS", completed.stdout)

    def test_unknown_commit_is_refused(self):
        artifact = valid_artifact()
        artifact["commit"] = "unknown"
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("commit", completed.stdout)

    def test_bench_error_is_refused(self):
        artifact = valid_artifact()
        artifact["error"] = "ipc timeout after 30000ms"
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("bench error", completed.stdout)

    def test_implicit_shared_buffer_claim_requires_proof(self):
        artifact = valid_artifact()
        artifact.pop("claims", None)
        artifact["shared_buffer"]["replies_ok"] = 5
        artifact["results"][1].pop("shared_buffer_used")
        artifact["results"][1].pop("shared_buffer_hits")
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("shared_buffer_used", completed.stdout)

    def test_zero_reply_claim_is_refused(self):
        artifact = valid_artifact()
        artifact["claims"] = ["shared-buffer"]
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("zero shared-buffer replies", completed.stdout)

    def test_losing_honest_measurement_is_accepted(self):
        artifact = valid_artifact()
        artifact["results"][0]["mean_from_batch_ms"] = 99.0
        artifact["results"][0]["summary"] = {"mean_from_batch_ms": 99.0}
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)

    def test_ring_zerocopy_fixture_is_accepted_with_allow_fixtures(self):
        completed = run_gate(
            "check",
            "--allow-fixtures",
            str(FIXTURES / "through-webview-ipc.ring-zerocopy.example.json"),
        )
        self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)
        self.assertIn("ACCEPTED", completed.stdout)

    def test_ring_claim_without_proof_fixture_is_refused(self):
        completed = run_gate(
            "check",
            "--allow-fixtures",
            str(FIXTURES / "through-webview-ipc.ring-claim-no-proof.example.json"),
        )
        self.assertNotEqual(completed.returncode, 0)
        self.assertIn("REFUSED", completed.stdout)
        self.assertIn("ring", completed.stdout)

    def test_ring_zerocopy_claim_with_proof_is_accepted(self):
        artifact = ring_artifact()
        artifact["claims"] = ["ring-zerocopy"]
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)

    def test_ring_claim_aliases_normalize(self):
        for alias in ("ring", "ring_zerocopy", "ringzerocopy"):
            artifact = ring_artifact()
            artifact["claims"] = [alias]
            with tempfile.TemporaryDirectory() as directory:
                path = write_tmp(directory, artifact)
                completed = run_gate("check", str(path))
                self.assertEqual(
                    completed.returncode, 0,
                    f"alias {alias}: {completed.stdout + completed.stderr}",
                )

    def test_ring_claim_with_default_transport_is_refused(self):
        artifact = ring_artifact()
        artifact["transport"] = "default"
        artifact["claims"] = ["ring-zerocopy"]
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("transport 'ring_zerocopy'", completed.stdout)

    def test_ring_claim_without_transport_is_refused(self):
        artifact = ring_artifact()
        del artifact["transport"]
        artifact["claims"] = ["ring-zerocopy"]
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("transport 'ring_zerocopy'", completed.stdout)

    def test_ring_zero_traffic_is_refused(self):
        artifact = ring_artifact()
        artifact["claims"] = ["ring-zerocopy"]
        artifact["ring"]["replies_ok"] = 0
        for entry in artifact["results"]:
            entry["ring_slot_hits"] = 0
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("zero ring replies", completed.stdout)

    def test_implicit_ring_claim_via_transport_requires_proof(self):
        artifact = ring_artifact()
        artifact.pop("claims", None)
        del artifact["ring"]
        for entry in artifact["results"]:
            del entry["ring_slot_hits"]
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("ring", completed.stdout)

    def test_implicit_ring_claim_via_traffic_requires_transport(self):
        artifact = valid_artifact()
        artifact.pop("claims", None)
        artifact["transport"] = "default"
        artifact["ring"] = {
            "replies_ok": 4,
            "replies_fallback": 0,
            "send_fallbacks": 0,
        }
        artifact["results"][1]["ring_slot_hits"] = 4
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("transport 'ring_zerocopy'", completed.stdout)

    def test_zero_copy_claim_uses_ring_path_on_ring_transport(self):
        artifact = ring_artifact()
        artifact["claims"] = ["zero-copy"]
        del artifact["shared_buffer"]
        for entry in artifact["results"]:
            del entry["shared_buffer_hits"]
            del entry["shared_buffer_used"]
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)

    def test_zero_copy_claim_uses_shared_buffer_path_on_default_transport(self):
        artifact = valid_artifact()
        artifact["claims"] = ["zero-copy"]
        artifact["shared_buffer"]["replies_ok"] = 10
        artifact["results"][1]["shared_buffer_hits"] = 5
        artifact["results"][1]["shared_buffer_used"] = True
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)

    def test_zero_copy_claim_without_any_evidence_is_refused(self):
        artifact = valid_artifact()
        artifact["claims"] = ["zero-copy"]
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            completed = run_gate("check", str(path))
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("zero shared-buffer replies", completed.stdout)

    def test_stamp_then_check_passes(self):
        artifact = valid_artifact()
        del artifact["run"]
        del artifact["commit"]
        with tempfile.TemporaryDirectory() as directory:
            src = write_tmp(directory, artifact)
            stamped = Path(directory) / "stamped.json"
            completed = run_gate(
                "stamp",
                str(src),
                "--out",
                str(stamped),
                "--run-id",
                "999",
                "--runner",
                "local",
                "--os",
                "testOS",
                "--commit",
                "deadbee",
            )
            self.assertEqual(completed.returncode, 0, completed.stderr)
            completed = run_gate("check", str(stamped))
            self.assertEqual(completed.returncode, 0, completed.stdout + completed.stderr)

    def test_verdict_out_marks_status(self):
        artifact = valid_artifact()
        with tempfile.TemporaryDirectory() as directory:
            path = write_tmp(directory, artifact)
            verdict = Path(directory) / "verdict.json"
            completed = run_gate("check", str(path), "--verdict-out", str(verdict))
            self.assertEqual(completed.returncode, 0)
            data = json.loads(verdict.read_text(encoding="utf-8"))
            self.assertEqual(data["status"], "accepted")
            self.assertEqual(data["artifacts"][0]["status"], "accepted")

        with tempfile.TemporaryDirectory() as directory:
            bad = write_tmp(directory, {"name": "bulk_bench", "results": []})
            verdict = Path(directory) / "verdict.json"
            completed = run_gate("check", str(bad), "--verdict-out", str(verdict))
            self.assertNotEqual(completed.returncode, 0)
            data = json.loads(verdict.read_text(encoding="utf-8"))
            self.assertEqual(data["status"], "refused")


if __name__ == "__main__":
    unittest.main()
