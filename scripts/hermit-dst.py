#!/usr/bin/env python3
"""Run the native Ethereum node integration test under Hermit's scheduler.

HERMIT_BIN selects the Hermit executable; GUEST_BIN reuses an already built test
executable. HERMIT_SRC optionally identifies the corresponding Hermit checkout.
Strict verification requires --verify-strict/--verify-json. The explicit standard
policy supports older Hermit versions and supplements their filtered log check
with exact stdout/stderr and normalized scheduler-event comparisons.
PMU preemption is enabled by default. The explicit --no-pmu profile uses clock-multiplier=0.002 to cancel Hermit's extra 500x
sequential clock scaling, which otherwise expires RPC deadlines during this test.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]
TEST = "hermit::hermit_native_node_transfers"
MARKER = "RETH_HERMIT_RESULT "
GUEST_PATH = "/tmp/reth-hermit-payload/guest"
BUILD_ENVIRONMENT = ["CFLAGS", "CXXFLAGS", "RUSTFLAGS", "CARGO_PROFILE_DEV_DEBUG",
                     "CARGO_PROFILE_DEV_SPLIT_DEBUGINFO", "CARGO_INCREMENTAL", "CARGO_TARGET_DIR"]
GUEST_ENV = {
    "HOME": "/tmp",
    "TMPDIR": "/tmp",
    "RAYON_NUM_THREADS": "2",
    "TZ": "UTC",
    "LANG": "C",
    "LC_ALL": "C",
    "RUST_BACKTRACE": "0",
}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def capture(command):
    try:
        result = subprocess.run(command, text=True, capture_output=True, check=False)
    except OSError:
        return None
    if result.returncode:
        return None
    return result.stdout.strip()


def source_metadata(path, output, name):
    revision = capture(["git", "-C", str(path), "rev-parse", "HEAD"])
    if revision is None:
        return None
    status = capture(["git", "-C", str(path), "status", "--porcelain"])
    patch = output / f"{name}.patch"
    with patch.open("wb") as destination:
        subprocess.run(
            ["git", "-C", str(path), "diff", "--binary", "HEAD"],
            stdout=destination,
            check=True,
        )
    result = {"path": str(path), "revision": revision, "status": status,
              "patch": patch.name, "patch_sha256": sha256(patch)}
    source_root = capture(["git", "-C", str(path), "rev-parse", "--show-toplevel"])
    lock = Path(source_root) / "Cargo.lock"
    if lock.is_file():
        snapshot = output / f"{name}-Cargo.lock"
        shutil.copyfile(lock, snapshot)
        result["cargo_lock"] = {"snapshot": snapshot.name, "sha256": sha256(snapshot)}
    return result


def executable(value):
    found = shutil.which(value)
    if not found:
        raise RuntimeError(f"executable not found: {value}")
    return Path(found).resolve()


def read_result(path):
    results = [line.partition(MARKER)[2] for line in path.read_text().splitlines()
               if MARKER in line]
    if len(results) != 1:
        raise RuntimeError(f"{path}: expected exactly one {MARKER.strip()} transcript")
    return json.loads(results[0])


class Campaign:
    def __init__(self, output, verification, build_environment):
        self.output = output
        self.verification = verification
        self.supported_options = set()
        self.commands = []
        self.results = []
        self.host_tmp = output / "host-tmp"
        self.host_tmp.mkdir()
        self.host_env = {"TMPDIR": str(self.host_tmp), "NO_COLOR": "1", **build_environment}

    def run(self, name, command, timeout):
        stdout = self.output / f"{name}.stdout"
        stderr = self.output / f"{name}.stderr"
        record = {"name": name, "argv": command, "cwd": str(ROOT),
                  "environment_overrides": self.host_env, "timeout_seconds": timeout,
                  "stdout": stdout.name, "stderr": stderr.name}
        self.commands.append(record)
        write_json(self.output / "commands.json", self.commands)
        shell = "env " + shlex.join([f"{k}={v}" for k, v in self.host_env.items()] + command)
        with (self.output / "commands.sh").open("a") as commands:
            commands.write(f"cd {shlex.quote(str(ROOT))}\n{shell} >"
                           f"{shlex.quote(str(stdout))} 2>{shlex.quote(str(stderr))}\n")
        print(f"{name}: running (timeout {timeout}s)", flush=True)
        started = time.monotonic()
        with stdout.open("wb") as out, stderr.open("wb") as err:
            process = subprocess.Popen(command, cwd=ROOT, stdout=out, stderr=err,
                                       env={**os.environ, **self.host_env}, start_new_session=True)
            try:
                status = process.wait(timeout=timeout)
            except (subprocess.TimeoutExpired, KeyboardInterrupt) as error:
                # Hermit can have tracees and a container supervisor; stop the whole group.
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
                record["timed_out_or_interrupted"] = True
                status = 124 if isinstance(error, subprocess.TimeoutExpired) else 130
        record.update(exit_code=status, elapsed_seconds=round(time.monotonic() - started, 3))
        write_json(self.output / "commands.json", self.commands)
        if status != 0:
            tail = stderr.read_text(errors="replace").splitlines()[-25:]
            print("\n".join(tail), file=sys.stderr)
            raise RuntimeError(f"{name} failed with status {status}; see {stderr}")
        diagnostics = stderr.read_text(errors="replace")
        log = self.output / f"{name}-hermit.log"
        if log.exists():
            diagnostics += log.read_text(errors="replace")
        if re.search(r"PMU validation failed|AmdSpecLockMapShouldBeDisabled|"
                     r"disabling.*(?:timeslice|preemption)|"
                     r"perf_event_open.*(?:failed|unavailable)|"
                     r"continuing with --max-timeslice=disabled",
                     diagnostics, re.IGNORECASE):
            raise RuntimeError(f"{name}: unreliable or unavailable PMU; see diagnostic logs")
        return stdout

    def verify(self, name, common, guest, timeout):
        options = ["--verify"]
        report = self.output / f"{name}.json"
        if self.verification == "strict":
            options += ["--verify-strict", f"--verify-json={report}"]
        if {"--keep-logs", "--verify-log-dir"} <= self.supported_options:
            log_dir = self.output / f"{name}-logs"
            log_dir.mkdir()
            options += ["--keep-logs", f"--verify-log-dir={log_dir}"]
        self.run(name, [common[0], "--log=info"] + common[1:] + options + ["--"] + guest, timeout)
        if self.verification == "strict":
            result = json.loads(report.read_text())
            if result.get("verified") is not True or result.get("bitwise_parity") is not True:
                raise RuntimeError(f"{name} did not establish strict parity; see {report}")
        else:
            result = {"verified": True, "bitwise_parity": None,
                      "policy": "standard", "reporter": "hermit-dst.py",
                      "evidence": "Hermit --verify exited successfully; internal logs are filtered"}
            write_json(report, result)
        return result

    def guest_run(self, name, common, guest, timeout):
        summary_path = self.output / f"{name}-summary.json"
        schedule_path = self.output / f"{name}-schedule.json"
        stdout = self.run(name, [common[0], "--log=warn", f"--log-file={self.output / (name + '-hermit.log')}"] + common[1:] + [f"--summary-json={summary_path}",
                          f"--record-preemptions-to={schedule_path}", "--"] + guest, timeout)
        summary = json.loads(summary_path.read_text())
        schedule = json.loads(schedule_path.read_text())
        # Only thread/operation order proves schedule variation. Ignore initial priorities,
        # addresses, timing, and any configuration metadata in the recording.
        events = [[event["dettid"], event["op"], event["count"]]
                  for event in schedule["global"]]
        if not events:
            raise RuntimeError(f"{name}: schedule contains no recorded events")
        write_json(self.output / f"{name}-schedule-events.json", events)
        return stdout, summary, events


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("seeds", nargs="*", type=int, default=[1, 2, 3],
                        help="scheduler seeds (default: 1 2 3; guest RNG seed stays 0)")
    parser.add_argument("--hermit-bin", default=os.environ.get("HERMIT_BIN", "hermit"))
    parser.add_argument("--guest-bin", default=os.environ.get("GUEST_BIN"))
    parser.add_argument("--native-output", type=Path,
                        help="also compare chain results with an existing native test stdout log")
    parser.add_argument("--output-dir", type=Path,
                        help="new artifact directory outside /tmp (default: target/hermit-dst)")
    parser.add_argument("--timeout", type=int, default=900,
                        help="wall-clock timeout for each Hermit invocation, seconds (default: 900)")
    parser.add_argument("--build-timeout", type=int, default=7200)
    parser.add_argument("--verification", choices=["strict", "standard"], default="strict",
                        help="standard explicitly allows filtered Hermit logs and adds an exact "
                             "stdout/stderr and event-order repeat check (default: strict)")
    parser.add_argument("--mdbx-safe4qemu", action="store_true",
                        help="append -DMDBX_SAFE4QEMU=1 to CFLAGS when building the test guest")
    parser.add_argument("--no-virtualize-cpuid", action="store_true",
                        help="explicitly expose host CPU features; records this portability limit")
    parser.add_argument("--no-pmu", action="store_true",
                        help="explicitly disable branch-counter preemption and RCB time")
    args = parser.parse_args()
    if any(seed < 0 or seed > 2**64 - 1 for seed in args.seeds):
        parser.error("seeds must fit an unsigned 64-bit integer")
    if len(set(args.seeds)) != len(args.seeds):
        parser.error("seeds must be distinct")
    if args.timeout <= 0 or args.build_timeout <= 0:
        parser.error("timeouts must be positive")
    hermit = executable(args.hermit_bin)
    if args.output_dir:
        output = args.output_dir.resolve()
        if output == Path("/tmp") or Path("/tmp") in output.parents:
            parser.error("Hermit's isolated /tmp would hide summary/preemption artifact paths")
        output.mkdir(parents=True, exist_ok=False)
    else:
        parent = ROOT / "target" / "hermit-dst"
        parent.mkdir(parents=True, exist_ok=True)
        output = Path(tempfile.mkdtemp(prefix="campaign-", dir=parent))
    print(f"Artifacts: {output}", flush=True)
    build_environment = {name: os.environ[name] for name in BUILD_ENVIRONMENT if name in os.environ}
    if args.mdbx_safe4qemu:
        build_environment["CFLAGS"] = (build_environment.get("CFLAGS", "")
                                      + " -DMDBX_SAFE4QEMU=1").strip()
    campaign = Campaign(output, args.verification, build_environment)
    common = [str(hermit), "run", "--backend=ptrace", "--base-env=minimal", "--workdir=/tmp",
              "--chaos", "--sched-heuristic=random", "--seed=0", "--rng-seed=0", "--fuzz-seed=0",
              "--epoch=2026-01-01T00:00:00Z"]
    if args.no_pmu:
        common += ["--preemption-timeout=disabled", "--no-rcb-time", "--clock-multiplier=0.002"]
    else:
        common += ["--max-timeslice=200000000", "--clock-multiplier=1",
                   "--panic-on-rcb-overshoot"]
    for name, value in GUEST_ENV.items():
        common += ["-e", f"{name}={value}"]
    if args.no_virtualize_cpuid:
        common.append("--no-virtualize-cpuid")
    hermit_source = Path(os.environ.get("HERMIT_SRC", hermit.parent))
    metadata = {
        "created_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "host": platform.uname()._asdict(),
        "cpu": capture(["lscpu"]),
        "reth": source_metadata(ROOT, output, "reth"),
        "hermit": {"path": str(hermit), "sha256": sha256(hermit),
                   "version": capture([str(hermit), "--version"]),
                   "source": source_metadata(hermit_source, output, "hermit")},
        "rustc": capture(["rustc", "-Vv"]),
        "cargo": capture(["cargo", "-V"]),
        "test": TEST,
        "scheduler_seeds": args.seeds,
        "guest_rng_seed": 0,
        "fuzz_seed": 0,
        "epoch": "2026-01-01T00:00:00Z",
        "guest_environment": GUEST_ENV,
        "virtualize_cpuid": not args.no_virtualize_cpuid,
        "preemption_timeout": "disabled" if args.no_pmu else 200000000,
        "rcb_time": not args.no_pmu,
        "clock_multiplier": 0.002 if args.no_pmu else 1,
        "build_environment": build_environment,
        "verification": args.verification,
        "comparison": ("--verify-strict (Hermit canonical INFO parity)"
                       if args.verification == "strict" else
                       "--verify (filtered internal logs) plus exact external stdout/stderr "
                       "and normalized scheduler-event repeat; no bitwise internal-log parity claim"),
        "source_sha256": {str(path.relative_to(ROOT)): sha256(path) for path in [
            ROOT / "Cargo.lock", ROOT / "Cargo.toml", Path(__file__).resolve(),
            ROOT / "crates/ethereum/node/tests/it/main.rs",
            ROOT / "crates/ethereum/node/tests/it/hermit.rs"]},
    }
    write_json(output / "metadata.json", metadata)
    try:
        for source in metadata["source_sha256"]:
            snapshot = output / "source" / source
            snapshot.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / source, snapshot)
        help_path = campaign.run("hermit-help", [str(hermit), "run", "--help"], args.timeout)
        campaign.supported_options = set(re.findall(
            r"^\s+(?:-[A-Za-z],\s+)?(--[a-z][a-z0-9-]*)", help_path.read_text(), re.MULTILINE))
        metadata["hermit"]["supported_options"] = sorted(campaign.supported_options)
        metadata["hermit"]["keeps_matched_verification_logs"] = (
            {"--keep-logs", "--verify-log-dir"} <= campaign.supported_options)
        write_json(output / "metadata.json", metadata)
        required = {"--verify", "--summary-json", "--record-preemptions-to"}
        if args.verification == "strict":
            required |= {"--verify-strict", "--verify-json"}
        missing = required - campaign.supported_options
        if missing:
            raise RuntimeError(f"Hermit lacks options required by {args.verification} policy: "
                               + ", ".join(sorted(missing)))
        campaign.verify("probe", common + [f"--sched-seed={args.seeds[0]}"],
                        ["/bin/true"], args.timeout)
        if args.guest_bin:
            guest = executable(args.guest_bin)
            metadata["guest_build"] = "supplied via GUEST_BIN/--guest-bin"
        else:
            build_log = campaign.run("build", ["cargo", "test", "--locked", "-p",
                                      "reth-node-ethereum", "--test", "it", "--no-run",
                                      "--message-format=json"], args.build_timeout)
            artifacts = set()
            for line in build_log.read_text().splitlines():
                message = json.loads(line)
                if (message.get("reason") == "compiler-artifact"
                        and message.get("target", {}).get("name") == "it"
                        and "test" in message.get("target", {}).get("kind", [])
                        and message.get("executable")):
                    artifacts.add(message["executable"])
            if len(artifacts) != 1:
                raise RuntimeError(f"expected one integration-test executable, found {artifacts}")
            guest = executable(artifacts.pop())
            metadata["guest_build"] = "Cargo compiler-artifact executable"
        metadata["guest"] = {"path": str(guest), "sha256": sha256(guest)}
        write_json(output / "metadata.json", metadata)
        # Older Hermit versions cannot resolve a file bind's program path; a directory bind
        # also limits the guest-visible payload to this exact executable.
        payload = output / "payload"
        payload.mkdir()
        try:
            os.link(guest, payload / "guest")
        except OSError:
            shutil.copy2(guest, payload / "guest")
        common += [f"--bind={payload}:/tmp/reth-hermit-payload"]
        guest_args = [GUEST_PATH, TEST, "--ignored", "--exact", "--nocapture", "--test-threads=1"]
        expected = read_result(args.native_output) if args.native_output else None
        if args.native_output:
            write_json(output / "native-result.json", expected)
            metadata["native_output"] = {"path": str(args.native_output.resolve()),
                                         "sha256": sha256(args.native_output)}
            write_json(output / "metadata.json", metadata)
        schedules = set()
        for seed in args.seeds:
            name = f"seed-{seed}"
            seed_args = common + [f"--sched-seed={seed}"]
            stdout, summary, events = campaign.guest_run(name, seed_args, guest_args, args.timeout)
            result = read_result(stdout)
            write_json(output / f"{name}-result.json", result)
            if expected is None:
                expected = result
            elif result != expected:
                raise RuntimeError(f"{name}: chain transcript differs from the baseline result")
            verification = campaign.verify(f"{name}-verify", seed_args, guest_args, args.timeout)
            if args.verification == "standard":
                repeated = f"{name}-repeat"
                _, _, repeated_events = campaign.guest_run(repeated, seed_args, guest_args, args.timeout)
                for stream in ["stdout", "stderr"]:
                    original_path = output / f"{name}.{stream}"
                    repeated_path = output / f"{repeated}.{stream}"
                    if original_path.read_bytes() != repeated_path.read_bytes():
                        raise RuntimeError(f"{name}: same-seed {stream} bytes differ on repeat")
                if events != repeated_events:
                    raise RuntimeError(f"{name}: same-seed scheduler-event sequence differs on repeat")
            canonical_schedule = json.dumps(events, sort_keys=True, separators=(",", ":"))
            schedule_hash = hashlib.sha256(canonical_schedule.encode()).hexdigest()
            schedules.add(schedule_hash)
            campaign.results.append({"seed": seed, "verified": verification["verified"],
                                     "verification": args.verification,
                                     "bitwise_parity": verification["bitwise_parity"],
                                     "exact_output_and_event_repeat": args.verification == "standard",
                                     "schedule_sha256": schedule_hash,
                                     "schedule_events": len(events),
                                     "sched_turns": summary["sched_turns"],
                                     "num_threads": summary["num_threads"],
                                     "result_sha256": sha256(output / f"{name}-result.json")})
            write_json(output / "results.json", campaign.results)
            print(f"{name}: {args.verification} verification passed, {summary['sched_turns']} scheduler turns, "
                  f"{summary['num_threads']} threads, schedule {schedule_hash[:12]}", flush=True)
        if len(args.seeds) > 1 and len(schedules) < 2:
            raise RuntimeError("seeds produced identical schedule records; schedule variation unproven")
        write_json(output / "status.json", {"status": "passed", "seeds": args.seeds,
                                           "verification": args.verification,
                                           "distinct_schedules": len(schedules)})
        print(f"PASS: {len(args.seeds)} seeds, {len(schedules)} distinct schedules, "
              f"identical chain transcript. Artifacts: {output}")
        return 0
    except (RuntimeError, OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        write_json(output / "status.json", {"status": "failed", "error": str(error)})
        print(f"FAIL: {error}\nArtifacts retained: {output}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
