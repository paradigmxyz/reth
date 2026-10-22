"""Smoke test chart generation using the locked chart dependencies."""

import csv
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

from PIL import Image


SCRIPTS = Path(__file__).resolve().parent


class ChartSmokeTests(unittest.TestCase):
    def test_candidate_without_chart_project(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            candidate = root / "candidate"
            candidate_scripts = candidate / ".github" / "scripts"
            candidate_scripts.mkdir(parents=True)
            shutil.copyfile(
                SCRIPTS / "bench-reth-charts.py",
                candidate_scripts / "bench-reth-charts.py",
            )
            self.assertFalse((candidate_scripts / "bench-reth-charts").exists())
            inputs = {}
            for name, offset in [("baseline", 0), ("feature", 500)]:
                paths = []
                for run in range(2):
                    path = root / f"{name}-{run}.csv"
                    with path.open("w", newline="") as stream:
                        writer = csv.writer(stream)
                        writer.writerow([
                            "block_number", "gas_used", "new_payload_latency",
                            "persistence_wait", "execution_cache_wait", "sparse_trie_wait",
                        ])
                        for block in range(20):
                            writer.writerow([
                                1000 + block, 10_000_000 + block * 100_000,
                                20_000 + block * 100 + offset + run * 50,
                                1000 + block, 200 + block, 300 + block,
                            ])
                    paths.append(str(path))
                inputs[name] = paths

            output = root / "charts"
            subprocess.run([
                "uv", "run", "--frozen", "--python", sys.executable,
                "--project", str(SCRIPTS / "bench-reth-charts"),
                "python", ".github/scripts/bench-reth-charts.py",
                "--feature", *inputs["feature"],
                "--baseline", *inputs["baseline"],
                "--output-dir", str(output),
            ], check=True, cwd=candidate)

            self.assertEqual(sorted(path.name for path in output.iterdir()), [
                "gas_vs_latency.png", "latency_throughput.png", "wait_breakdown.png",
            ])
            for path in output.iterdir():
                with self.subTest(chart=path.name), Image.open(path) as image:
                    self.assertEqual(image.format, "PNG")
                    self.assertGreater(image.width, 0)
                    self.assertGreater(image.height, 0)
                    image.verify()


if __name__ == "__main__":
    unittest.main()
