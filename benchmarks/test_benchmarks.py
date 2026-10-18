"""Behavioral checks for the benchmark adapters and suite failure reporting."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
BINARY = HERE / ".build" / "benchmark"


@unittest.skipUnless(BINARY.exists(), "run build.py first")
class BenchmarkTests(unittest.TestCase):
    def test_all_formats_round_trip_and_answer_queries(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "suite"
            report = Path(directory) / "report.md"
            result = subprocess.run([sys.executable, str(HERE / "run.py"), "--scale-divisor", "1000",
                                     "--repeats", "1", "--output", str(out), "--report", str(report)],
                                    capture_output=True, text=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            data = json.loads((out / "raw.json").read_text())
            self.assertEqual(data["status"], "complete; all checks passed")
            self.assertEqual(report.read_text(), (out / "report.md").read_text())
            for profile in ("requests", "many-generators", "mixed"):
                matches = [d["count"] for d in data["oracles"][profile]["search-transaction"]]
                self.assertTrue(any(matches), profile)
                self.assertIn(0, matches, profile)

    def test_deadline_preserves_an_incomplete_report(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory) / "suite"
            report = Path(directory) / "report.md"
            report.write_text("Previous successful report\n")
            result = subprocess.run([sys.executable, str(HERE / "run.py"), "--budget-seconds", "0.001",
                                     "--output", str(out), "--report", str(report)],
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertEqual(json.loads((out / "raw.json").read_text())["status"], "INCOMPLETE")
            self.assertIn("**INCOMPLETE**", (out / "report.md").read_text())
            self.assertEqual(report.read_text(), "Previous successful report\n")

    def test_ftr_reader_rejects_truncated_chunks_and_missing_terminator(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "requests.ftr"
            command = [str(BINARY), "write", "ftr", "requests", str(path), "100"]
            subprocess.run(command, check=True, capture_output=True, text=True, timeout=10)
            data = path.read_bytes()
            for length in (1, len(data) // 2, len(data) - 1):
                with self.subTest(length=length):
                    path.write_bytes(data[:length])
                    command[1] = "read"
                    result = subprocess.run(command, capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertIn("benchmark:", result.stderr)


if __name__ == "__main__":
    unittest.main()
