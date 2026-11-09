import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class ParseTests(unittest.TestCase):
    def run_parser(self, suite, name, passed, ignored=()):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            report = {
                "name": suite,
                "testCases": {
                    "1": {"name": name, "summaryResult": {"pass": passed}},
                },
            }
            (root / "report.json").write_text(json.dumps(report))
            # JSON is also valid YAML, which keeps the fixtures dependency-free.
            (root / "expected.yaml").write_text(json.dumps({suite: ["fixture-reth"]}))
            (root / "ignored.yaml").write_text(json.dumps({suite: list(ignored)}))
            return subprocess.run(
                [
                    sys.executable,
                    str(Path(__file__).with_name("parse.py")),
                    str(root / "report.json"),
                    "--exclusion", str(root / "expected.yaml"),
                    "--ignored", str(root / "ignored.yaml"),
                ],
                capture_output=True,
                text=True,
                check=False,
            )

    def test_expected_failures_match_both_new_suites(self):
        for suite in ("eels/consume-enginex", "eels/consume-wirex"):
            with self.subTest(suite=suite):
                result = self.run_parser(suite, "fixture-reth", False)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, "Success.\n")

    def test_wirex_bal_variants_match_shared_expectations(self):
        for suffix in ("bal_withheld", "bal_served"):
            with self.subTest(suffix=suffix):
                result = self.run_parser("eels/consume-wirex", f"fixture-reth-{suffix}", False)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, "Success.\n")

    def test_wirex_unexpected_passes_keep_variant_names(self):
        for suffix in ("bal_withheld", "bal_served"):
            with self.subTest(suffix=suffix):
                name = f"fixture-reth-{suffix}"
                result = self.run_parser("eels/consume-wirex", name, True)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(result.stdout, f"Unexpected Passes:\n  {name}\n")

    def test_wirex_unexpected_failures_keep_variant_names(self):
        name = "another-fixture-reth-bal_served"
        result = self.run_parser("eels/consume-wirex", name, False)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(result.stdout, f"Unexpected Failures:\n  {name}\n")

    def test_other_suites_and_suffixes_are_matched_exactly(self):
        for suite, name in (
            ("eels/consume-enginex", "fixture-reth-bal_served"),
            ("eels/consume-wirex", "fixture-reth-another_variant"),
        ):
            with self.subTest(suite=suite, name=name):
                result = self.run_parser(suite, name, False)
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertEqual(result.stdout, f"Unexpected Failures:\n  {name}\n")

    def test_wirex_variants_can_be_ignored_individually(self):
        ignored = ("fixture-reth-bal_withheld",)
        result = self.run_parser("eels/consume-wirex", ignored[0], True, ignored)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            result.stdout,
            "Ignored Tests:\n  Passed (1 tests):\n    fixture-reth-bal_withheld\n\nSuccess.\n",
        )
        result = self.run_parser("eels/consume-wirex", "fixture-reth-bal_served", True, ignored)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(result.stdout, "Unexpected Passes:\n  fixture-reth-bal_served\n")


if __name__ == "__main__":
    unittest.main()
