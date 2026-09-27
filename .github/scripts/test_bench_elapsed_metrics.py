"""Regression tests for measured elapsed metrics and the shared Slack renderer.

Run with: uv run python -m unittest discover -s .github/scripts
"""

import csv
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("bench_summary", SCRIPTS / "bench-reth-summary.py")
summary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(summary)


class ElapsedMetricsTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def run_fixture(self, name, execution_us, duration_ms, blocks=2):
        directory = self.root / name
        directory.mkdir()
        path = directory / "combined_latency.csv"
        with path.open("w", newline="") as f:
            writer = csv.DictWriter(f, fieldnames=[
                "block_number", "gas_used", "new_payload_latency", "total_latency",
                "persistence_wait", "execution_cache_wait", "sparse_trie_wait",
            ])
            writer.writeheader()
            for number in range(100, 100 + blocks):
                writer.writerow({
                    "block_number": number, "gas_used": 1_000_000,
                    "new_payload_latency": execution_us, "total_latency": execution_us,
                    "persistence_wait": 0, "execution_cache_wait": 0, "sparse_trie_wait": 0,
                })
        if duration_ms is not None:
            (directory / "report.json").write_text(json.dumps({"run_stats": {
                "duration_ms": duration_ms, "total_gas": blocks * 1_000_000,
                "total_blocks": blocks, "start_block": 100, "end_block": 99 + blocks,
            }}))
        return str(path)

    def compare(self, baseline_paths, feature_paths):
        b_runs = [summary.parse_combined_csv(path) for path in baseline_paths]
        f_runs = [summary.parse_combined_csv(path) for path in feature_paths]
        b = summary.compute_point_stats(b_runs)
        f = summary.compute_point_stats(f_runs)
        ci = summary.compute_ci_stats(b_runs, f_runs)
        summary.add_elapsed_stats(baseline_paths, feature_paths, b_runs, f_runs, b, f, ci)
        return b, f, ci, summary.compute_changes(b, f, ci)

    def render_rows(self, b, f, changes):
        result = subprocess.run([
            "node", "-e",
            "const u=require(process.argv[1]); let s=''; "
            "process.stdin.on('data', d=>s+=d); "
            "process.stdin.on('end', ()=>{const x=JSON.parse(s); "
            "process.stdout.write(JSON.stringify({rows:u.metricRows(x),verdict:u.verdict(x.changes)}));});",
            str(SCRIPTS / "bench-utils.js"),
        ], input=json.dumps({"baseline": {"stats": b}, "feature": {"stats": f}, "changes": changes}),
            text=True, capture_output=True, check=True)
        return json.loads(result.stdout)

    def test_persistence_savings_survive_slower_execution(self):
        baseline = [self.run_fixture(f"b{i}", 10_000, 1000) for i in range(2)]
        feature = [self.run_fixture(f"f{i}", 20_000, 500) for i in range(2)]
        b, f, ci, changes = self.compare(baseline, feature)
        self.assertEqual(b["wall_clock_s"], 1.0)  # Mean per run, not the sum of two runs.
        self.assertEqual(f["wall_clock_s"], 0.5)
        self.assertEqual(b["end_to_end_mgas_s"], 2.0)
        self.assertEqual(f["end_to_end_mgas_s"], 4.0)
        self.assertEqual(changes["mean"]["sig"], "bad")
        self.assertEqual(changes["mgas_s"]["sig"], "bad")
        self.assertEqual(changes["wall_clock"]["sig"], "good")
        self.assertEqual(changes["end_to_end_mgas_s"]["sig"], "good")
        self.assertEqual(changes["wall_clock"]["pct"], -50.0)
        self.assertEqual(changes["end_to_end_mgas_s"]["pct"], 100.0)
        rendered = self.render_rows(b, f, changes)
        rows = {row["label"]: row for row in rendered["rows"]}
        self.assertEqual(rows["Wall Clock"]["baseline"], "1.00s")
        self.assertEqual(rows["End-to-end Mgas/s"]["feature"], "4.00")
        self.assertIn("❌", rows["Execution Mean"]["change"])
        self.assertIn("✅", rows["Wall Clock"]["change"])
        self.assertEqual(rendered["verdict"]["label"], "Mixed Results")
        markdown = summary.generate_comparison_table(b, f, ci, "owner/repo", "base", "base", "feat", "sha")
        self.assertIn("| Wall Clock | 1.00s | 0.50s |", markdown)
        self.assertIn("| End-to-end Mgas/s | 2.00 | 4.00 |", markdown)

    def test_average_run_throughput_not_ratio_of_averages(self):
        baseline = [self.run_fixture("b1", 10_000, 1000), self.run_fixture("b2", 10_000, 3000)]
        feature = [self.run_fixture("f1", 10_000, 2000), self.run_fixture("f2", 10_000, 2000)]
        b, f, ci, changes = self.compare(baseline, feature)
        self.assertEqual(b["wall_clock_s"], 2)
        self.assertAlmostEqual(b["end_to_end_mgas_s"], (2 + 2 / 3) / 2)
        self.assertAlmostEqual(ci["wall_clock_ci_s"], 1)
        self.assertAlmostEqual(changes["wall_clock"]["ci_pct"], 50)
        self.assertEqual(changes["wall_clock"]["sig"], "neutral")

    def test_run_count_does_not_inflate_elapsed_time(self):
        baseline = [self.run_fixture(f"b{i}", 10_000, 1000) for i in range(2)]
        feature = [self.run_fixture(f"f{i}", 10_000, 1000) for i in range(3)]
        b, f, ci, changes = self.compare(baseline, feature)
        self.assertEqual(b["wall_clock_s"], f["wall_clock_s"])
        self.assertEqual(changes["wall_clock"]["pct"], 0)

    def test_single_run_elapsed_verdict_is_informational(self):
        b, f, ci, changes = self.compare(
            [self.run_fixture("b", 10_000, 1000)], [self.run_fixture("f", 10_000, 500)],
        )
        for name in ("wall_clock", "end_to_end_mgas_s"):
            self.assertTrue(changes[name]["informational"])
            self.assertEqual(changes[name]["sig"], "neutral")
        self.assertEqual(self.render_rows(b, f, changes)["verdict"]["label"], "No Difference")

    def test_missing_report_does_not_fabricate_wall_clock(self):
        baseline = [self.run_fixture("b1", 10_000, 1000), self.run_fixture("b2", 10_000, None)]
        feature = [self.run_fixture(f"f{i}", 10_000, 500) for i in range(2)]
        b, f, ci, changes = self.compare(baseline, feature)
        self.assertIsNone(b["wall_clock_s"])
        self.assertIsNone(f["end_to_end_mgas_s"])
        self.assertNotIn("wall_clock", changes)
        rows = {row["label"]: row for row in self.render_rows(b, f, changes)["rows"]}
        self.assertEqual(rows["Wall Clock"]["baseline"], "n/a")
        markdown = summary.generate_comparison_table(b, f, ci, "owner/repo", "base", "base", "feat", "sha")
        self.assertIn("| Wall Clock | n/a | n/a |", markdown)

    def test_invalid_duration_rejected(self):
        for i, duration in enumerate((0, -1, float("nan"), float("inf"), True, "1000")):
            with self.subTest(duration=duration):
                path = self.run_fixture(f"bad{i}", 10_000, duration)
                with self.assertRaisesRegex(ValueError, "Invalid measured duration"):
                    summary.load_elapsed_run(path, summary.parse_combined_csv(path))

    def test_mismatched_report_rejected(self):
        path = self.run_fixture("bad", 10_000, 1000)
        rows = summary.parse_combined_csv(path)
        rows[0]["gas_used"] += 1
        with self.assertRaisesRegex(ValueError, "do not match"):
            summary.load_elapsed_run(path, rows)


if __name__ == "__main__":
    unittest.main()
