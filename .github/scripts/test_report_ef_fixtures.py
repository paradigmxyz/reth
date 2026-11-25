"""Check that diagnostic replays preserve the original fixture verdicts."""

import io
import json
import runpy
import tempfile
import unittest
from contextlib import chdir, redirect_stdout
from pathlib import Path
from unittest.mock import patch

report = runpy.run_path(str(Path(__file__).with_name("report-ef-fixtures.py")))["report"]


class ReplayTests(unittest.TestCase):
    def test_only_failures_replay_in_the_original_mode(self):
        cases = {"passed": {}, "known": {}, "unexpected": {}}
        results = [{"name": name, "error": "", "pass": name == "passed"} for name in cases]
        expected = {"eels/consume-enginex": ["known"], "ef-test-runner/blocktest": ["known"]}
        with tempfile.TemporaryDirectory() as tmp, chdir(tmp), patch.dict("os.environ", clear=True):
            root = Path("fixtures")
            root.mkdir()
            (root / "fixture-paths").write_text(json.dumps(dict.fromkeys(cases, "mixed.json")))
            (root / "mixed.json").write_text(json.dumps(cases))
            for mode in ("blocktest", "enginetest", "enginetest-sequential"):
                with self.subTest(mode=mode), patch("subprocess.run") as runner:
                    log = io.StringIO()
                    replayed = []

                    def run(args, **kwargs):
                        self.assertEqual(args[1], mode.removesuffix("-sequential"))
                        self.assertEqual("--bal-sequential" in args, mode == "enginetest-sequential")
                        self.assertIs(kwargs["stderr"], log)
                        [name] = json.loads(args[-1].read_bytes())
                        replayed.append(name)
                        return runner.return_value

                    runner.side_effect = run
                    runner.return_value.returncode = 0
                    with redirect_stdout(log):
                        self.assertEqual(report(mode, 1, results, expected), 1)
                        self.assertEqual(report(mode, 1, results[:2], expected), 0)
                        self.assertEqual(report(mode, 0, results[:1], expected), 0)
                    self.assertEqual(replayed, ["unexpected", "known", "known"])
                    self.assertEqual(
                        [line for line in log.getvalue().splitlines() if line.startswith("::")],
                        ["::group::FAIL unexpected", "::endgroup::", "::group::XFAIL known",
                         "::endgroup::", "::group::XFAIL known", "::endgroup::"],
                    )
