import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("bench-execution.py")
spec = importlib.util.spec_from_file_location("execution", SCRIPT)
execution = importlib.util.module_from_spec(spec)
spec.loader.exec_module(execution)


class ExecutionTest(unittest.TestCase):
    def test_phase_manifest_and_attempt_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "run-order.txt").write_text("baseline-1\nfeature-1\nbaseline-2\nfeature-2\n")
            (root / "summary.json").write_text(json.dumps({"benchmark_id": "bench-e2e-123-2"}))
            (root / "report-feature-1.json").write_text(json.dumps({"metadata": {"git-sha": "abc123", "git-ref": "feature"}}))
            (root / "clickhouse-run-id-feature-1.txt").write_text("00000000-0000-4000-8000-000000000001")
            env = {"GITHUB_REPOSITORY": "tempoxyz/tempo", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2", "BENCH_PR": "42"}
            row = execution.build_manifest(root, env, "failure")
            self.assertEqual(row["execution_id"], "tempoxyz:tempo:123:2")
            self.assertEqual(row["pr_number"], 42)
            self.assertEqual(row["status"], "failure")
            phases = json.loads(row["phases"])
            self.assertEqual(len(phases), 4)
            self.assertFalse(phases[0]["recorded"])
            self.assertEqual(phases[1]["commit"], "abc123")
            self.assertTrue(phases[1]["report_id"])
            self.assertEqual(phases[3]["report_id"], "")

    def test_upload_has_no_ddl_and_no_retry(self):
        row = {"execution_id": "paradigmxyz:reth:123:1"}
        env = {"CLICKHOUSE_HOST": "https://example.invalid?user=url-user&password=url-secret", "CLICKHOUSE_USER": "writer", "CLICKHOUSE_PASSWORD": "secret"}
        with patch.object(execution, "urlopen", side_effect=TimeoutError) as request:
            with self.assertRaises(TimeoutError):
                execution.upload(row, env)
            self.assertEqual(request.call_count, 1)
            sent = request.call_args.args[0]
            self.assertNotIn("secret", sent.full_url)
            self.assertIn("INSERT", sent.full_url)
            self.assertNotIn("CREATE", sent.full_url)

    def test_reth_phases_without_run_order_and_no_pr(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "baseline-1").mkdir()
            (root / "feature-1").mkdir()
            (root / "feature-1" / "combined_latency.csv").write_text("latency\n1\n")
            env = {"GITHUB_REPOSITORY": "paradigmxyz/reth", "GITHUB_RUN_ID": "9"}
            row = execution.build_manifest(root, env, "success")
            self.assertEqual(row["pr_number"], 0)
            self.assertEqual([p["label"] for p in json.loads(row["phases"])], ["baseline-1", "feature-1"])
            self.assertTrue(json.loads(row["phases"])[1]["recorded"])


if __name__ == "__main__":
    unittest.main()
