#!/usr/bin/env python3
"""Tests for bench-call-summary.py.

Run with: python3 -m unittest discover -s .github/scripts
"""

from __future__ import annotations

import csv
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


def _load_module():
    path = Path(__file__).resolve().with_name("bench-call-summary.py")
    spec = importlib.util.spec_from_file_location("bench_call_summary", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


summary_script = _load_module()

HEAD_HASH = "0x" + "ab" * 32
PASSES = 3


def digest_for(record_index: int) -> str:
    return "0x" + f"{record_index:064x}"


def write_run(
    directory: Path,
    methods: list[str],
    latency_us: int,
    digests: dict[int, str] | None = None,
    kinds: dict[int, str] | None = None,
    head_hash: str = HEAD_HASH,
    usage_usec: int = 2_000_000,
    nondeterministic: list[int] | None = None,
    record_latency_us: dict[int, int] | None = None,
) -> Path:
    """Write one synthetic `bench call` run directory."""
    directory.mkdir(parents=True, exist_ok=True)
    digests = digests or {}
    kinds = kinds or {}
    record_latency_us = record_latency_us or {}
    records = list(enumerate(methods, start=1))

    def latency(index: int) -> int:
        return record_latency_us.get(index, latency_us + index)

    with (directory / "responses.ndjson").open("w") as f:
        for index, method in records:
            f.write(
                json.dumps(
                    {
                        "record_index": index,
                        "method": method,
                        "kind": kinds.get(index, "ok"),
                        "digest": digests.get(index, digest_for(index)),
                        "len": 1024 + index,
                    }
                )
                + "\n"
            )

    with (directory / "record_timings.csv").open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["record_index", "method", "pass", "latency_us", "status"])
        for index, method in records:
            for pass_index in range(1, PASSES + 1):
                writer.writerow([index, method, pass_index, latency(index), "ok"])

    with (directory / "requests.csv").open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(["offset_ms", "record_index", "method", "latency_us", "status"])
        offset = 0
        for repeat in range(4):
            for index, method in records:
                offset += 10
                status = "ok" if not (repeat == 3 and index == 1) else "rpc_error"
                writer.writerow([offset, index, method, latency(index), status])

    report = {
        "chain_id": 1,
        "head": 25_490_000,
        "head_hash": head_hash,
        "closed_loop_rps": 1_000_000 / latency_us,
        "dropped": 0,
        "nondeterministic": nondeterministic or [],
        "methods": {
            method: {"closed_loop_rps": 1_000_000 / latency_us, "dropped": 0}
            for method in set(methods)
        },
    }
    (directory / "report.json").write_text(json.dumps(report))

    requests_ok = len(records) * PASSES + len(records) * 4 - 1
    (directory / "cpu.json").write_text(
        json.dumps(
            {
                "usage_usec_start": 0,
                "usage_usec_end": usage_usec,
                "usage_usec_delta": usage_usec,
                "requests_total": requests_ok + 1,
                "requests_ok": requests_ok,
            }
        )
    )
    return directory


class CallSummaryTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.work = Path(self._tmp.name)
        self.addCleanup(self._tmp.cleanup)

    def run_summary(self, extra_args: list[str] | None = None) -> tuple[dict, str]:
        summary_path = self.work / "summary.json"
        comment_path = self.work / "comment.md"
        argv = [
            "--baseline-dir",
            str(self.work / "baseline-1"),
            str(self.work / "baseline-2"),
            "--feature-dir",
            str(self.work / "feature-1"),
            str(self.work / "feature-2"),
            "--output-summary",
            str(summary_path),
            "--output-markdown",
            str(comment_path),
            "--class",
            "call",
            "--rps",
            "100",
            "--duration",
            "120s",
            "--passes",
            "20",
            "--concurrency",
            "16",
            "--warmup-seconds",
            "60",
            "--run-pairs",
            "2",
        ]
        argv.extend(extra_args or [])
        self.assertEqual(summary_script.main(argv), 0)
        with summary_path.open() as f:
            return json.load(f), comment_path.read_text()

    def write_arms(
        self,
        methods: list[str],
        feature_digests: dict[int, str] | None = None,
        baseline_2_digests: dict[int, str] | None = None,
        feature_head_hash: str = HEAD_HASH,
    ) -> None:
        write_run(self.work / "baseline-1", methods, 1_000)
        write_run(self.work / "baseline-2", methods, 1_010, digests=baseline_2_digests)
        write_run(
            self.work / "feature-1",
            methods,
            1_005,
            digests=feature_digests,
            head_hash=feature_head_hash,
        )
        write_run(
            self.work / "feature-2",
            methods,
            1_015,
            digests=feature_digests,
            head_hash=feature_head_hash,
        )

    def test_matching_runs_report_no_mismatches(self):
        self.write_arms(["eth_call"] * 8)
        summary, comment = self.run_summary()

        self.assertEqual(summary["mode"], "call")
        self.assertEqual(summary["parity"]["status"], "matched")
        totals = summary["parity"]["feature"]["totals"]
        self.assertEqual(totals["content_mismatch"], 0)
        self.assertEqual(totals["kind_mismatch"], 0)
        self.assertEqual(totals["matched"], 16)
        self.assertEqual(summary["changes"]["parity"]["sig"], "neutral")
        self.assertIn("matched", comment)
        self.assertIn("mode: call", comment)
        # Latency and CPU metrics are present for the whole corpus.
        for key in ("mean_ms", "p50_ms", "record_median_ms", "cpu_ms_per_request"):
            self.assertIn(key, summary["baseline"]["stats"])
            self.assertIn(key, summary["feature"]["stats"])

    def test_feature_content_mismatch_fails_the_verdict(self):
        self.write_arms(["eth_call"] * 8, feature_digests={3: "0x" + "ff" * 32})
        summary, comment = self.run_summary()

        self.assertEqual(summary["parity"]["status"], "mismatch")
        self.assertEqual(summary["changes"]["parity"]["sig"], "bad")
        totals = summary["parity"]["feature"]["totals"]
        self.assertEqual(totals["content_mismatch"], 2)
        self.assertEqual(totals["kind_mismatch"], 0)
        per_method = summary["parity"]["feature"]["methods"]["eth_call"]
        self.assertEqual(per_method["divergent_records"], [3])
        self.assertIn("eth_call", comment)
        self.assertIn("records 3", comment)

    def test_kind_mismatch_is_reported_separately(self):
        self.write_arms(["eth_call"] * 8)
        write_run(
            self.work / "feature-1",
            ["eth_call"] * 8,
            1_005,
            kinds={2: "rpc_error"},
        )
        summary, _ = self.run_summary()

        totals = summary["parity"]["feature"]["totals"]
        self.assertEqual(totals["kind_mismatch"], 1)
        self.assertEqual(summary["parity"]["status"], "mismatch")

    def test_unstable_record_is_excluded_from_parity(self):
        # Record 5 differs between the baseline runs, so its feature mismatch is
        # not judged; the other records still match.
        self.write_arms(
            ["eth_call"] * 8,
            feature_digests={5: "0x" + "ee" * 32},
            baseline_2_digests={5: "0x" + "dd" * 32},
        )
        summary, comment = self.run_summary()

        self.assertEqual(summary["parity"]["status"], "matched")
        self.assertEqual(summary["parity"]["excluded"], [5])
        self.assertEqual(summary["parity"]["feature"]["totals"]["excluded"], 2)
        self.assertEqual(summary["changes"]["parity"]["sig"], "neutral")
        self.assertNotIn("informational", summary["changes"]["parity"])
        self.assertIn("unstable in the baseline, excluded (records 5)", comment)

    def test_stable_record_regression_survives_an_unstable_record(self):
        # Record 2 is noisy in the baseline; record 1 agrees across baselines and
        # changes in the feature, which must still fail parity.
        self.write_arms(
            ["eth_call"] * 8,
            feature_digests={1: "0x" + "ff" * 32},
            baseline_2_digests={2: "0x" + "dd" * 32},
        )
        summary, comment = self.run_summary()

        self.assertEqual(summary["parity"]["status"], "mismatch")
        self.assertEqual(summary["changes"]["parity"]["sig"], "bad")
        self.assertEqual(summary["parity"]["excluded"], [2])
        per_method = summary["parity"]["feature"]["methods"]["eth_call"]
        self.assertEqual(per_method["divergent_records"], [1])
        self.assertEqual(per_method["content_mismatch"], 2)
        self.assertIn("records 1", comment)

    def test_feature_only_nondeterminism_on_a_stable_record_is_a_mismatch(self):
        self.write_arms(["eth_call"] * 8)
        write_run(self.work / "feature-1", ["eth_call"] * 8, 1_005, nondeterministic=[4])
        summary, comment = self.run_summary()

        self.assertEqual(summary["parity"]["status"], "mismatch")
        self.assertEqual(summary["changes"]["parity"]["sig"], "bad")
        totals = summary["parity"]["feature"]["totals"]
        self.assertEqual(totals["nondeterministic"], 1)
        self.assertEqual(totals["content_mismatch"], 0)
        self.assertIn(4, summary["parity"]["feature"]["methods"]["eth_call"]["divergent_records"])
        self.assertIn("nondeterministic in the feature only", comment)

    def test_baseline_nondeterminism_only_excludes_that_record(self):
        self.write_arms(["eth_call"] * 8, feature_digests={4: "0x" + "ee" * 32})
        write_run(self.work / "baseline-1", ["eth_call"] * 8, 1_000, nondeterministic=[4])
        summary, _ = self.run_summary()

        self.assertEqual(summary["parity"]["status"], "matched")
        self.assertEqual(summary["parity"]["excluded"], [4])

    def test_all_records_unstable_is_inconclusive(self):
        self.write_arms(
            ["eth_call"] * 2,
            baseline_2_digests={1: "0x" + "aa" * 32, 2: "0x" + "bb" * 32},
        )
        summary, comment = self.run_summary()

        self.assertEqual(summary["parity"]["status"], "inconclusive")
        self.assertTrue(summary["changes"]["parity"]["informational"])
        self.assertIn("inconclusive", comment)

    def test_record_median_interval_follows_run_to_run_noise(self):
        # Baseline runs at 10 and 20 ms, feature runs both at 20 ms: the point
        # estimate says +33% but the runs disagree by 100%, so it is not a verdict.
        methods = ["eth_call"] * 8
        write_run(self.work / "baseline-1", methods, 10_000)
        write_run(self.work / "baseline-2", methods, 20_000)
        write_run(self.work / "feature-1", methods, 20_000)
        write_run(self.work / "feature-2", methods, 20_000)
        summary, _ = self.run_summary()

        change = summary["changes"]["record_median"]
        self.assertEqual(change["sig"], "neutral")
        self.assertGreater(change["ci_pct"], 10.0)
        self.assertAlmostEqual(change["aa_pct"], 100.0, delta=1.0)

    def test_record_median_regression_is_not_hidden_by_record_spread(self):
        # Every run repeats the same three records; the feature doubles each of
        # them, which is a 100% slowdown no matter how spread the records are.
        methods = ["eth_call"] * 3
        baseline = {1: 1_000, 2: 2_000, 3: 100_000}
        feature = {1: 2_000, 2: 4_000, 3: 200_000}
        write_run(self.work / "baseline-1", methods, 1_000, record_latency_us=baseline)
        write_run(self.work / "baseline-2", methods, 1_000, record_latency_us=baseline)
        write_run(self.work / "feature-1", methods, 1_000, record_latency_us=feature)
        write_run(self.work / "feature-2", methods, 1_000, record_latency_us=feature)
        summary, _ = self.run_summary()

        change = summary["changes"]["record_median"]
        self.assertEqual(change["sig"], "bad")
        self.assertAlmostEqual(change["pct"], 100.0, delta=1.0)
        self.assertLess(change["ci_pct"], 1.0)

    def test_mixed_method_corpus_reports_per_method(self):
        methods = ["eth_call", "eth_call", "debug_traceCall", "debug_traceCall"]
        self.write_arms(methods)
        summary, comment = self.run_summary()

        self.assertEqual(sorted(summary["methods"]), ["debug_traceCall", "eth_call"])
        for entry in summary["methods"].values():
            self.assertIn("mean_ms", entry["baseline"])
            self.assertIn("mean", entry["changes"])
            self.assertEqual(entry["parity"]["content_mismatch"], 0)
        self.assertIn("### Per-method", comment)
        self.assertIn("`debug_traceCall`", comment)

    def test_single_method_corpus_omits_per_method_table(self):
        self.write_arms(["eth_call"] * 8)
        summary, comment = self.run_summary()

        self.assertEqual(summary["methods"], {})
        self.assertNotIn("### Per-method", comment)

    def test_head_hash_mismatch_is_refused(self):
        self.write_arms(["eth_call"] * 8, feature_head_hash="0x" + "cd" * 32)
        with self.assertRaises(SystemExit) as ctx:
            self.run_summary()
        self.assertIn("different chain tips", str(ctx.exception))

    def test_nested_call_report_schema_is_read(self):
        self.write_arms(["eth_call"] * 4)
        for run in ("baseline-1", "baseline-2", "feature-1", "feature-2"):
            report_path = self.work / run / "report.json"
            flat = json.loads(report_path.read_text())
            nested = {
                "metadata": {"scenario": "call-replay"},
                "call": {
                    "identity": {"chain_id": 1, "head": 25_490_000, "head_hash": flat["head_hash"]},
                    "closed_loop_rps": flat["closed_loop_rps"],
                    "dropped": 0,
                    "nondeterministic": [],
                    "nondeterministic_total": 0,
                    "methods": {
                        method: {"closed_loop": {"rps": entry["closed_loop_rps"]}, "dropped": 0}
                        for method, entry in flat["methods"].items()
                    },
                },
            }
            report_path.write_text(json.dumps(nested))
        summary, _ = self.run_summary()

        self.assertIn("closed_loop_rps", summary["baseline"]["stats"])
        self.assertIn("closed_loop_rps", summary["feature"]["stats"])
        self.assertEqual(summary["parity"]["status"], "matched")

        nested_feature = json.loads((self.work / "feature-1" / "report.json").read_text())
        nested_feature["call"]["identity"]["head_hash"] = "0x" + "cd" * 32
        (self.work / "feature-1" / "report.json").write_text(json.dumps(nested_feature))
        with self.assertRaises(SystemExit):
            self.run_summary()

    def test_record_answered_in_one_arm_only_is_a_divergence(self):
        self.write_arms(["eth_call"] * 4)
        for run in ("feature-1", "feature-2"):
            path = self.work / run / "responses.ndjson"
            kept = [line for line in path.read_text().splitlines() if json.loads(line)["record_index"] != 2]
            path.write_text("\n".join(kept) + "\n")
        summary, comment = self.run_summary()

        self.assertEqual(summary["parity"]["status"], "mismatch")
        totals = summary["parity"]["feature"]["totals"]
        self.assertEqual(totals["missing"], 2)
        self.assertEqual(totals["mismatched"], 2)
        self.assertIn(2, summary["parity"]["feature"]["methods"]["eth_call"]["divergent_records"])
        self.assertIn("answered in one arm only", comment)

    def test_corpus_metadata_is_included(self):
        self.write_arms(["eth_call"] * 8)
        meta_path = self.work / "corpus.meta.json"
        meta_path.write_text(
            json.dumps(
                {
                    "source": "custom",
                    "name": "captured",
                    "class": "call",
                    "methods": ["eth_call"],
                    "records": 8,
                    "records_per_method": {"eth_call": 8},
                    "tip": 25_490_000,
                    "tip_hash": HEAD_HASH,
                }
            )
        )
        summary, comment = self.run_summary(["--corpus-meta", str(meta_path)])

        self.assertEqual(summary["corpus"]["name"], "captured")
        self.assertEqual(summary["corpus"]["source"], "custom")
        self.assertIn("captured", comment)


if __name__ == "__main__":
    unittest.main()
