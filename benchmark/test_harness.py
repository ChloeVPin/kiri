import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
HARNESS = ROOT / "benchmark" / "harness.py"


class HarnessSchemaTests(unittest.TestCase):
    def test_warmups_are_retained_separately_from_measured_runs(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "result.json"
            subprocess.run(
                [
                    sys.executable,
                    str(HARNESS),
                    "--name",
                    "test",
                    "--runs",
                    "2",
                    "--warmups",
                    "2",
                    "--timeout-seconds",
                    "5",
                    "--output",
                    str(output),
                    "--",
                    sys.executable,
                    "-c",
                    "print('ok')",
                ],
                cwd=ROOT,
                check=True,
                capture_output=True,
                text=True,
            )
            result = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(len(result["warmups"]["samples_ms"]), 2)
            self.assertEqual(result["warmups"]["requested"], 2)
            self.assertEqual(result["summary"]["count"], 2)
            self.assertEqual(len(result["samples_ms"]), 2)


if __name__ == "__main__":
    unittest.main()
