"""Public Cargo-JSON fixtures; no Rust compiler or third-party modules needed."""

import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / "lint-fuzz-primitives.py"
SPEC = importlib.util.spec_from_file_location("lint_fuzz_primitives", SCRIPT)
guard = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(guard)


def diagnostic(source="NativeMutex", start=10, lint=guard.LINTS[0], path="crates/demo/src/lib.rs", message="use of a disallowed type `std::sync::Mutex`"):
    return {"reason": "compiler-message", "message": {
        "level": "warning", "code": {"code": lint}, "message": message,
        "spans": [{"file_name": path, "is_primary": True, "byte_start": start,
                   "byte_end": start + len(source), "text": [{"text": source,
                   "highlight_start": 1, "highlight_end": len(source) + 1}]}],
    }}


def artifact(root):
    return {"reason": "compiler-artifact", "manifest_path": str(root / "crates/demo/Cargo.toml")}


def lines(*messages, success=True):
    return [json.dumps(item) + "\n" for item in (*messages, {"reason": "build-finished", "success": success})]


class GuardTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.baseline = self.root / "baseline.json"
        self.input = self.root / "cargo.jsonl"
        self.output = self.root / "report.json"
        self.write_baseline([])

    def write_baseline(self, entries):
        self.baseline.write_text(json.dumps({"schema_version": 1, "profiles": {"fuzz": entries, "all-features": []}}))

    def check(self, messages, all_features=False):
        self.input.write_text("".join(messages))
        with patch.object(guard, "ROOT", self.root), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            return guard.main(["--baseline", str(self.baseline), "--check-report", str(self.input), "--report", str(self.output)] + (["--all-features"] if all_features else []))

    def reviewed(self, *messages, count=None):
        counts, _, _ = guard.read_diagnostics(lines(*messages), self.root)
        entries = guard.entries(counts)
        for entry in entries:
            entry["reason"] = "Native FFI boundary: bridge to an OS worker."
            if count is not None:
                entry["count"] = count
        self.write_baseline(entries)

    def test_unexpected_diagnostic_fails_and_reports(self):
        self.assertEqual(self.check(lines(diagnostic())), 1)
        report = json.loads(self.output.read_text())
        self.assertEqual(report["new"][0]["count"], 1)
        self.assertFalse(report["passed"])

    def test_duplicate_library_and_test_messages_count_once(self):
        item = diagnostic()
        self.reviewed(item)
        self.assertEqual(self.check(lines(item, item)), 0)
        self.assertEqual(json.loads(self.output.read_text())["entries"][0]["count"], 1)

    def test_identical_new_use_exceeds_multiplicity(self):
        self.reviewed(diagnostic())
        self.assertEqual(self.check(lines(diagnostic(), diagnostic(start=50))), 1)
        report = json.loads(self.output.read_text())
        self.assertEqual(report["entries"][0]["count"], 2)
        self.assertEqual(report["new"][0]["count"], 1)

    def test_line_movement_is_allowed(self):
        self.reviewed(diagnostic())
        self.assertEqual(self.check(lines(diagnostic(start=100))), 0)

    def test_deletion_requires_removing_stale_permissions(self):
        self.reviewed(diagnostic(), count=2)
        self.assertEqual(self.check(lines(diagnostic())), 1)
        self.assertEqual(json.loads(self.output.read_text())["stale"][0]["count"], 1)
        self.assertEqual(self.check(lines(artifact(self.root))), 1)
        self.assertEqual(json.loads(self.output.read_text())["stale"][0]["count"], 2)
        self.write_baseline([])
        self.assertEqual(self.check(lines(artifact(self.root))), 0)

    def test_changed_alias_source_or_message_is_new(self):
        self.reviewed(diagnostic())
        for item in (diagnostic(source="OtherAlias"), diagnostic(message="different resolved native type")):
            with self.subTest(item=item):
                self.assertEqual(self.check(lines(item)), 1)

    def test_alias_highlights_and_multiline_source(self):
        item = diagnostic()
        item["message"]["spans"][0]["text"] = [
            {"text": "    Alias::<", "highlight_start": 5, "highlight_end": 13},
            {"text": "       T>", "highlight_start": 8, "highlight_end": 10},
        ]
        counts, _, _ = guard.read_diagnostics(lines(item), self.root)
        self.assertEqual(next(iter(counts))[2], "Alias::<\nT>")

    def test_every_target_lint_is_checked(self):
        for lint in guard.LINTS:
            with self.subTest(lint=lint):
                self.assertEqual(self.check(lines(diagnostic(lint=lint))), 1)

    def test_external_dependencies_are_not_baseline_debt(self):
        item = diagnostic(path=str(self.root.parent / "registry/lib.rs"))
        self.assertEqual(self.check(lines(item, artifact(self.root))), 0)
        self.assertEqual(json.loads(self.output.read_text())["entries"], [])

    def test_external_macro_span_is_mapped_to_each_workspace_invocation(self):
        first, second = diagnostic(start=100), diagnostic(start=200)
        for item in (first, second):
            callsite = item["message"]["spans"][0]
            generated = diagnostic(path="/rustc/std/thread_local.rs")["message"]["spans"][0]
            generated["expansion"] = {"span": callsite}
            item["message"]["spans"] = [generated]
        counts, _, _ = guard.read_diagnostics(lines(first, first, second), self.root)
        self.assertEqual(sum(counts.values()), 2)
        self.assertEqual(next(iter(counts))[0], "crates/demo/src/lib.rs")

    def test_local_macro_definition_does_not_merge_distinct_calls(self):
        first, second = diagnostic(start=10), diagnostic(start=10)
        first["message"]["spans"][0]["expansion"] = {"span": diagnostic(source="make_native!()", start=100)["message"]["spans"][0]}
        second["message"]["spans"][0]["expansion"] = {"span": diagnostic(source="make_native!()", start=200)["message"]["spans"][0]}
        counts, _, _ = guard.read_diagnostics(lines(first, first, second), self.root)
        self.assertEqual(sum(counts.values()), 2)
        self.assertEqual(next(iter(counts))[2], "make_native!()")

    def test_non_target_warning_is_not_a_violation(self):
        self.assertEqual(self.check(lines(diagnostic(lint="unused_imports"))), 0)

    def test_compiler_error_fails_even_with_success_marker(self):
        item = diagnostic(lint=None)
        item["message"]["level"] = "error"
        self.assertEqual(self.check(lines(item)), 1)
        self.assertEqual(json.loads(self.output.read_text())["compiler_errors"], [item["message"]["message"]])

    def test_failed_build_fails_without_diagnostics(self):
        self.assertEqual(self.check(lines(success=False)), 1)
        self.assertFalse(json.loads(self.output.read_text())["build_success"])

    def test_malformed_truncated_or_empty_reports_fail(self):
        bad_reports = [[], ["not json\n"], ["{}\n"], ["[]\n"], lines(),
                       [json.dumps(diagnostic())], lines(artifact(self.root)) + ["{}\n"],
                       lines({"reason": "compiler-artifact"}),
                       [json.dumps({"reason": "build-finished", "success": 1})]]
        for report in bad_reports:
            with self.subTest(report=report):
                self.assertEqual(self.check(report), 1)

    def test_malformed_spans_fail_and_replace_old_report(self):
        self.output.write_text('{"passed": true}')
        for change in ({"byte_start": True}, {"text": []}, {"is_primary": False},
                       {"text": [{"text": "Alias", "highlight_start": 0, "highlight_end": 5}]}):
            with self.subTest(change=change):
                item = diagnostic()
                item["message"]["spans"][0].update(change)
                self.assertEqual(self.check(lines(item)), 1)
                report = json.loads(self.output.read_text())
                self.assertFalse(report["passed"])
                self.assertIn("error", report)

    def test_invalid_baseline_rejected(self):
        self.reviewed(diagnostic())
        good = json.loads(self.baseline.read_text())
        for key, value in (("reason", ""), ("count", True), ("count", 0), ("path", "../escape.rs"), ("lint", "other")):
            with self.subTest(key=key, value=value):
                entry = dict(good["profiles"]["fuzz"][0], **{key: value})
                self.write_baseline([entry])
                self.assertEqual(self.check(lines(diagnostic())), 1)
        self.write_baseline(good["profiles"]["fuzz"] * 2)
        self.assertEqual(self.check(lines(diagnostic())), 1)

    def test_executes_builtin_clippy_with_absolute_config(self):
        config = self.root / "config"
        config.mkdir()
        (config / "clippy.toml").write_text("")
        completed = type("Completed", (), {"returncode": 0, "stdout": "".join(lines(artifact(self.root)))})()
        with patch.object(guard, "ROOT", self.root), patch.dict(os.environ, {"CARGO": "/custom/cargo"}), patch.object(guard.subprocess, "run", return_value=completed) as run, contextlib.redirect_stdout(io.StringIO()):
            result = guard.main(["--baseline", str(self.baseline), "--config-dir", str(config), "--toolchain", "+nightly"])
        self.assertEqual(result, 0)
        self.assertEqual(run.call_args.args[0], ["/custom/cargo", "+nightly", "clippy", "--workspace", "--all-targets", "--features", "fuzz", "--locked", "--message-format=json", "--", "--force-warn", guard.LINTS[0], "--force-warn", guard.LINTS[1], "--force-warn", guard.LINTS[2]])
        self.assertEqual(run.call_args.kwargs["env"]["CLIPPY_CONF_DIR"], str(config.resolve()))
        self.assertEqual(run.call_args.kwargs["cwd"], self.root)

    def test_profiles_have_independent_permissions(self):
        self.reviewed(diagnostic())
        self.assertEqual(self.check(lines(diagnostic())), 0)
        self.assertEqual(self.check(lines(diagnostic()), all_features=True), 1)
        baseline = json.loads(self.baseline.read_text())
        baseline["profiles"]["all-features"] = baseline["profiles"]["fuzz"]
        self.baseline.write_text(json.dumps(baseline))
        self.assertEqual(self.check(lines(diagnostic()), all_features=True), 0)
        self.assertEqual(json.loads(self.output.read_text())["profile"], "all-features")

    def test_missing_profile_does_not_imply_empty_baseline(self):
        self.baseline.write_text('{"schema_version":1,"profiles":{"fuzz":[]}}')
        self.assertEqual(self.check(lines(artifact(self.root)), all_features=True), 1)

    def test_all_features_uses_its_own_cargo_switch(self):
        config = self.root / "config"
        config.mkdir()
        (config / "clippy.toml").write_text("")
        completed = type("Completed", (), {"returncode": 0, "stdout": "".join(lines(artifact(self.root)))})()
        with patch.object(guard, "ROOT", self.root), patch.object(guard.subprocess, "run", return_value=completed) as run, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(guard.main(["--baseline", str(self.baseline), "--config-dir", str(config), "--all-features"]), 0)
        command = run.call_args.args[0]
        self.assertEqual(command[1:8], ["clippy", "--workspace", "--all-targets", "--all-features", "--locked", "--message-format=json", "--"])
        self.assertNotIn("--features", command)

    def test_cargo_exit_failure_is_not_hidden(self):
        config = self.root / "config"
        config.mkdir()
        (config / "clippy.toml").write_text("")
        completed = type("Completed", (), {"returncode": 101, "stdout": "".join(lines(artifact(self.root)))})()
        with patch.object(guard, "ROOT", self.root), patch.object(guard.subprocess, "run", return_value=completed), contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(guard.main(["--baseline", str(self.baseline), "--config-dir", str(config)]), 1)


if __name__ == "__main__":
    unittest.main()
