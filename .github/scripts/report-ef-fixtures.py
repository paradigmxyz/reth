"""Report EF runner verdicts against the expected failures for this fixture format."""

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import yaml


def flatten(entries):
    for entry in entries:
        if isinstance(entry, list):
            yield from flatten(entry)
        else:
            yield entry


def replay_failures(mode, failures):
    root = Path("fixtures")
    paths = json.loads((root / "fixture-paths").read_bytes())
    args = ["./ef-test-runner", mode.removesuffix("-sequential"), "--workers", "1", "--progress"]
    if mode == "enginetest-sequential":
        args.append("--bal-sequential")

    # Files can contain several cases. Give each process exactly one so engine worker logs
    # stay with the fixture that produced them.
    with tempfile.TemporaryDirectory() as tmp:
        fixture = Path(tmp) / "fixture.json"
        for label, name in failures:
            title = name.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
            print(f"::group::{label} {title}", flush=True)
            try:
                case = json.loads((root / paths[name]).read_bytes())[name]
                with fixture.open("w") as out:
                    json.dump({name: case}, out)
                replay = subprocess.run([*args, fixture], stdout=subprocess.DEVNULL, stderr=sys.stdout)
                if replay.returncode == 0:
                    print("Replay passed. The original verdict still counts.")
                elif replay.returncode != 1:
                    print(f"Replay runner exited with status {replay.returncode}.")
            except (KeyError, OSError, ValueError) as err:
                print(f"Could not replay fixture: {err}")
            finally:
                print("::endgroup::", flush=True)


def report(mode, status, results, expected):
    if mode == "blocktest":
        suite, fixture_format = "ef-test-runner/blocktest", "blockchain_test"
    else:
        suite, fixture_format = "eels/consume-enginex", "blockchain_test_engine"

    # Hive stores Engine X IDs with a client suffix. Keep all fork and test parameters intact.
    allowed = {
        name.removesuffix("-reth").replace("-blockchain_test_engine_x", "-" + fixture_format, 1)
        for name in flatten(expected[suite])
    }
    if not isinstance(results, list) or not results:
        sys.exit("No fixture verdicts were reported.")
    if any(type(row["pass"]) is not bool for row in results):
        sys.exit("Invalid fixture verdict in runner output.")
    failed = [row for row in results if not row["pass"]]
    if status != int(bool(failed)):
        sys.exit(f"Runner exit {status} disagrees with the fixture verdicts.")

    unexpected = [row for row in failed if row["name"] not in allowed]
    known = [row for row in failed if row["name"] in allowed]
    failures = []
    for label, color, rows in (("FAIL", 31, unexpected), ("XFAIL", 33, known)):
        for row in rows:
            print(f"\033[{color}m{label}\033[0m {row['name']}: {row['error']}")
            failures.append((label, row["name"]))

    passed = len(results) - len(failed)
    print(f"\033[32mPASS {passed}\033[0m | \033[33mXFAIL {len(known)}\033[0m | \033[31mFAIL {len(unexpected)}\033[0m")
    if "GITHUB_STEP_SUMMARY" in os.environ:
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as out:
            print(f"**{mode} / {os.environ['FIXTURE_SHARD']}**: {passed} passed, "
                  f"{len(known)} expected failures, {len(unexpected)} unexpected failures.", file=out)
    if failures:
        try:
            replay_failures(mode, failures)
        except (OSError, ValueError) as err:
            print(f"Could not prepare fixture replays: {err}")
    return int(bool(unexpected))


if __name__ == "__main__":
    mode, status, result_file, expected_file = sys.argv[1:]
    sys.exit(report(mode, int(status), json.loads(Path(result_file).read_bytes()),
                    yaml.safe_load(Path(expected_file).read_text())))
