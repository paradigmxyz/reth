#!/usr/bin/env python3
"""Observe node resources without wrapping the node or changing its signals.

Run outside the node's systemd scope, from before launch until after scope stop.
RSS values and fault/CPU counters are sampled, so the final interval can be missed.
Cgroup memory includes file cache and every process in the scope (including samply).
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time


def process_sample(path):
    status = {}
    for line in (path / "status").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in ("VmRSS", "RssAnon", "RssFile", "RssShmem", "VmHWM"):
            status[key + "_bytes"] = int(value.split()[0]) * 1024
    # comm may contain spaces and parentheses; fields after its closing ')' start at 3.
    fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
    return {
        "pid": int(path.name),
        "starttime_ticks": int(fields[19]),
        "minor_faults": int(fields[7]),
        "major_faults": int(fields[9]),
        "user_ticks": int(fields[11]),
        "system_ticks": int(fields[12]),
        **status,
    }


def find_scope(scope):
    result = subprocess.run(
        ["systemctl", "show", "--property=ControlGroup", "--value", scope],
        text=True, capture_output=True, check=False, timeout=5,
    )
    path = result.stdout.strip()
    return Path("/sys/fs/cgroup") / path.lstrip("/") if path else None


def sample_cgroup(path, handles):
    values = {}
    for name in ("memory.current", "memory.peak", "memory.max"):
        try:
            # Keep descriptors open through teardown; kernels may still reject reads
            # after cgroup removal. A failed read never replaces the last valid peak.
            if name not in handles:
                handles[name] = (path / name).open()
            handles[name].seek(0)
            value = handles[name].read().strip()
            values[name] = int(value) if value != "max" else value
        except OSError:
            pass
    return values


def run(args):
    output = Path(args.output_dir)
    binary = Path(args.binary).resolve()
    handles = {}
    scope = None
    seen_processes = {}
    cgroup_peaks = {}
    errors = set()
    started = time.time_ns() // 1_000_000
    samples = 0
    last_cgroup_sample = None
    try:
        with (output / "resource-samples.jsonl").open("w") as stream:
            while True:
                stopping = Path(args.stop_file).exists()
                if scope is None:
                    scope = find_scope(args.scope)
                observed_ms = time.time_ns() // 1_000_000
                processes = []
                cgroup = {}
                if scope is not None:
                    cgroup = sample_cgroup(scope, handles)
                    try:
                        pids = (scope / "cgroup.procs").read_text().split()
                    except OSError:
                        pids = []
                    for pid in pids:
                        path = Path("/proc") / pid
                        try:
                            # Match the executable, not sudo/systemd-run/samply or comm.
                            if not os.path.samefile(path / "exe", binary):
                                continue
                            item = process_sample(path)
                            processes.append(item)
                            key = f"{pid}:{item['starttime_ticks']}"
                            previous = seen_processes.setdefault(key, {"first": item, "first_unix_ms": observed_ms, "peaks": {}})
                            previous["last"] = item
                            previous["last_unix_ms"] = observed_ms
                            for name, value in item.items():
                                if name.endswith("_bytes"):
                                    previous["peaks"][name] = max(value, previous["peaks"].get(name, 0))
                        except (OSError, ValueError, IndexError) as exc:
                            errors.add(type(exc).__name__)
                if cgroup:
                    last_cgroup_sample = {"unix_ms": observed_ms, "values": cgroup}
                for name, value in cgroup.items():
                    if isinstance(value, int):
                        cgroup_peaks[name] = max(value, cgroup_peaks.get(name, 0))
                stream.write(json.dumps({"unix_ms": time.time_ns() // 1_000_000,
                                         "processes": processes, "cgroup": cgroup}) + "\n")
                stream.flush()
                samples += 1
                if stopping:
                    break
                time.sleep(args.interval)
    finally:
        for handle in handles.values():
            handle.close()
        summary = {
            "scope": args.scope, "binary": str(binary), "interval_seconds": args.interval,
            "clock_ticks_per_second": os.sysconf("SC_CLK_TCK"), "started_unix_ms": started,
            "ended_unix_ms": time.time_ns() // 1_000_000, "samples": samples,
            "processes": seen_processes, "cgroup_observed_peaks": cgroup_peaks,
            "sampling_errors": sorted(errors), "last_cgroup_sample": last_cgroup_sample,
            "coverage": "launch through scope shutdown; observations may miss the final process interval",
            "cgroup_coverage": "entire scope including file cache and profiler; last readable memory.peak retained",
        }
        (output / "resource-summary.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scope", required=True)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--stop-file", required=True)
    parser.add_argument("--interval", type=float, default=0.25)
    args = parser.parse_args()
    if args.interval <= 0:
        parser.error("--interval must be positive")
    run(args)
