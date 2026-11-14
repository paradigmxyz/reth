#!/usr/bin/env python3
"""Reject native primitive uses reported by built-in Clippy lints.

Run both the default fuzz profile and --all-features on CI's pinned toolchain.
Normal/native Clippy never loads this separate policy. Tokio is deliberately
not forbidden; shared native I/O and metadata boundaries are not simulation.

Clippy sees resolved local references and macro expansions, not operations
hidden inside dependencies or backend-specialized methods on generic lock_api
containers. Hash seeds, native I/O and Tokio scheduling require separate audits.
"""

import argparse
from collections import Counter
import io
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
LINTS = tuple(f"clippy::disallowed_{kind}" for kind in ("types", "methods", "macros"))
FIELDS = ("path", "lint", "source", "message")


def workspace_path(name, root):
    if not isinstance(name, str) or not name.strip():
        raise ValueError("compiler paths must be nonempty strings")
    path = Path(name)
    try:
        return (root / path).resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return None


def read_diagnostics(lines, root):
    """Use byte locations only for build deduplication; source text survives edits."""
    counts, seen, errors = Counter(), set(), set()
    finished, evidence = None, False
    for number, line in enumerate(lines, 1):
        if not line.strip():
            continue
        if finished is not None:
            raise ValueError(f"report has data after build-finished at line {number}")
        try:
            item = json.loads(line)
            reason = item["reason"]
            if reason == "build-finished":
                finished = item["success"]
                if type(finished) is not bool:
                    raise ValueError("build-finished success must be boolean")
            elif reason == "compiler-artifact":
                evidence |= workspace_path(item["manifest_path"], root) is not None
            elif reason == "compiler-message":
                diagnostic = item["message"]
                message, spans = diagnostic["message"], diagnostic["spans"]
                if not isinstance(message, str) or not isinstance(spans, list):
                    raise ValueError("invalid compiler diagnostic")
                code = diagnostic.get("code") or {}
                lint = code.get("code")
                if diagnostic["level"] == "error":
                    errors.add(message)
                primaries = [span for span in spans if span["is_primary"]]
                if lint in LINTS and not primaries:
                    raise ValueError("target lint is missing a primary source span")
                for span in primaries:
                    # Retain definition AND invocation locations: separate calls to a
                    # local macro can share one primary definition span.
                    workspace_spans, visited = [], set()
                    while True:
                        if id(span) in visited:
                            raise ValueError("cyclic macro expansion span")
                        visited.add(id(span))
                        path = workspace_path(span["file_name"], root)
                        if path is not None:
                            workspace_spans.append((path, span))
                        if not span.get("expansion"):
                            break
                        span = span["expansion"]["span"]
                    if not workspace_spans:
                        continue
                    path, span = workspace_spans[-1]
                    evidence = True
                    if lint not in LINTS:
                        continue
                    highlighted = []
                    for part in span["text"]:
                        text, begin, end = part["text"], part["highlight_start"], part["highlight_end"]
                        if not isinstance(text, str) or type(begin) is not int or type(end) is not int or not 1 <= begin <= end <= len(text) + 1:
                            raise ValueError("invalid source highlight range")
                        highlighted.append(text[begin - 1:end - 1])
                    source = "\n".join(highlighted).strip()
                    if not source or not message.strip():
                        raise ValueError("target lint is missing highlighted source text or message")
                    start, end = span["byte_start"], span["byte_end"]
                    if type(start) is not int or type(end) is not int or not 0 <= start <= end:
                        raise ValueError("invalid primary span byte range")
                    locations = []
                    for source_path, source_span in workspace_spans:
                        begin, finish = source_span["byte_start"], source_span["byte_end"]
                        if type(begin) is not int or type(finish) is not int or not 0 <= begin <= finish:
                            raise ValueError("invalid expansion span byte range")
                        locations.append((source_path, begin, finish))
                    location = (tuple(locations), lint, message)
                    if location not in seen:
                        seen.add(location)
                        counts[(path, lint, source, message)] += 1
            elif reason != "build-script-executed":
                raise ValueError(f"unknown Cargo message reason: {reason!r}")
        except (KeyError, TypeError, AttributeError, json.JSONDecodeError) as error:
            raise ValueError(f"malformed Cargo JSON at line {number}: {error}") from error
    if finished is None:
        raise ValueError("report is truncated: missing build-finished")
    if finished and not evidence:
        raise ValueError("report has no workspace compiler messages or artifacts")
    return counts, sorted(errors), finished


def entries(counts):
    result = []
    for key, count in sorted(counts.items()):
        entry = dict(zip(FIELDS, key), count=count)
        result.append(entry)
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--all-features", action="store_true", help="scan all features instead of default features plus fuzz")
    parser.add_argument("--config-dir", type=Path, default=ROOT / ".config/fuzz-primitives")
    parser.add_argument("--toolchain", help="optional Rust toolchain, e.g. nightly")
    parser.add_argument("--report", type=Path, help="write diagnostic counts and check result as JSON")
    parser.add_argument("--check-report", type=Path, help="check Cargo JSON-lines output without running Cargo")
    args = parser.parse_args(argv)
    profile = "all-features" if args.all_features else "fuzz"
    try:
        returncode = 0
        if args.check_report:
            with args.check_report.open() as report:
                counts, errors, success = read_diagnostics(report, ROOT)
        else:
            config = args.config_dir.resolve()
            if not (config / "clippy.toml").is_file():
                raise ValueError(f"missing Clippy config: {config / 'clippy.toml'}")
            command = [os.environ.get("CARGO", "cargo")]
            if args.toolchain:
                command.append("+" + args.toolchain.lstrip("+"))
            command += ["clippy", "--workspace", "--all-targets"]
            command += ["--all-features"] if args.all_features else ["--features", "fuzz"]
            command += ["--locked", "--message-format=json", "--"]
            for lint in LINTS:
                command += ["--force-warn", lint]
            completed = subprocess.run(command, cwd=ROOT, env={**os.environ, "CLIPPY_CONF_DIR": str(config)}, stdout=subprocess.PIPE, text=True)
            returncode = completed.returncode
            counts, errors, success = read_diagnostics(io.StringIO(completed.stdout), ROOT)
        totals = Counter()
        for (_, lint, _, _), count in counts.items():
            totals[lint] += count
        print(f"profile: {profile}")
        for lint in LINTS:
            print(f"{lint}: {totals[lint]} remaining")
        for entry in entries(counts):
            print(f"{entry['path']}: {entry['lint']} x{entry['count']}: {entry['source']!r}: {entry['message']}")
        for error in errors:
            print(f"compiler error: {error}", file=sys.stderr)
        passed = success and not errors and not returncode and not counts
        if args.report:
            args.report.write_text(json.dumps({"schema_version": 1, "profile": profile, "entries": entries(counts), "compiler_errors": errors, "build_success": success, "cargo_returncode": returncode, "passed": passed}, indent=2) + "\n")
        print(f"{sum(counts.values())} violations; build {'succeeded' if success and not returncode and not errors else 'failed'}")
        return 0 if passed else 1
    except (OSError, ValueError) as error:
        print(f"fuzz primitive guard: {error}", file=sys.stderr)
        if args.report:
            try:
                args.report.write_text(json.dumps({"schema_version": 1, "profile": profile, "passed": False, "error": str(error)}, indent=2) + "\n")
            except OSError as report_error:
                print(f"cannot write report: {report_error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
