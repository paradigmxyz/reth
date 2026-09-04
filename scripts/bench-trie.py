#!/usr/bin/env python3
r"""Compare two sparse trie benchmark executables using alternating ABBA runs.

Build the baseline and candidate with the same compiler and profile, copying each
executable before rebuilding. For example:

    cargo bench --locked --profile profiling -p reth-trie-sparse --bench state_root --no-run
    python3 scripts/bench-trie.py /tmp/state-root-baseline /tmp/state-root-candidate \
        --output bench-work/trie-comparison --threads 1,4 --cpus 0-3 --samples 100

The benchmark uses synthetic sparse storage tries with keccak-distributed keys
and RLP-encoded U256 values. Fixture setup, input allocation, and cloning are
excluded. Each executable validates every root against Alloy HashBuilder.

Each CSV in runs/ contains the benchmark's per-case sample median and quartiles,
not individual sample observations. summary.csv reports the median and quartiles
of these per-run medians. It does not pool samples or claim confidence intervals.
Run on an idle machine; this script does not stop other workloads.
"""

import argparse
import csv
import datetime
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import shutil
import statistics
import subprocess
import sys


FIELDS = ["case", "samples", "median_ns", "p25_ns", "p75_ns", "root"]
ORDER = ["baseline", "candidate", "candidate", "baseline"]


def positive_integer(value):
    number = int(value)
    if number <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return number


def parse_threads(value):
    try:
        threads = [positive_integer(item) for item in value.split(",")]
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected comma-separated thread counts") from error
    if len(set(threads)) != len(threads):
        raise argparse.ArgumentTypeError("thread counts must be distinct")
    return threads


def cpu_list(value):
    cpus = set()
    try:
        for item in value.split(","):
            limits = [int(part) for part in item.split("-")]
            if len(limits) == 1:
                cpus.add(limits[0])
            elif len(limits) == 2 and limits[0] <= limits[1]:
                cpus.update(range(limits[0], limits[1] + 1))
            else:
                raise ValueError("invalid range")
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected CPU list such as 0-3 or 0,2,4,6") from error
    if not cpus or min(cpus) < 0:
        raise argparse.ArgumentTypeError("CPU numbers must be nonnegative")
    return sorted(cpus)


def executable(value):
    path = Path(value).resolve()
    if not path.is_file() or not os.access(path, os.X_OK):
        raise argparse.ArgumentTypeError(f"not an executable file: {path}")
    return path


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def parse_result(output):
    reader = csv.DictReader(io.StringIO(output))
    if reader.fieldnames != FIELDS:
        raise ValueError(f"unexpected benchmark CSV fields: {reader.fieldnames}")
    results = {}
    for row in reader:
        case = row["case"]
        if case in results:
            raise ValueError(f"duplicate case: {case}")
        for field in FIELDS[1:-1]:
            row[field] = int(row[field])
        if row["samples"] <= 0 or not 0 <= row["p25_ns"] <= row["median_ns"] <= row["p75_ns"]:
            raise ValueError(f"invalid measurements: {row}")
        results[case] = row
    if not results:
        raise ValueError("benchmark returned no cases; check --filter")
    return results


def summarize(records, output):
    grouped = {}
    for record in records:
        key = (record["threads"], record["case"])
        grouped.setdefault(key, {"baseline": [], "candidate": []})[record["mode"]].append(
            record["median_ns"]
        )
    fields = [
        "threads", "case", "runs_per_mode", "samples_per_run", "baseline_median_ns",
        "baseline_p25_ns", "baseline_p75_ns", "candidate_median_ns", "candidate_p25_ns",
        "candidate_p75_ns", "speedup", "reduction_percent",
    ]
    with output.open("w", newline="") as destination:
        writer = csv.DictWriter(destination, fieldnames=fields)
        writer.writeheader()
        for (threads, case), modes in sorted(grouped.items()):
            row = {
                "threads": threads,
                "case": case,
                "runs_per_mode": len(modes["baseline"]),
                "samples_per_run": records[0]["samples"],
            }
            for mode, medians in modes.items():
                lower, _, upper = statistics.quantiles(medians, n=4, method="inclusive")
                row[f"{mode}_median_ns"] = statistics.median(medians)
                row[f"{mode}_p25_ns"] = lower
                row[f"{mode}_p75_ns"] = upper
            baseline = row["baseline_median_ns"]
            candidate = row["candidate_median_ns"]
            row["speedup"] = round(baseline / candidate, 4) if candidate else "inf"
            row["reduction_percent"] = round(100 * (1 - candidate / baseline), 2) if baseline else 0
            writer.writerow(row)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("baseline", type=executable)
    parser.add_argument("candidate", type=executable)
    parser.add_argument("--output", type=Path, required=True, help="new directory for metadata and CSV files")
    parser.add_argument("--threads", type=parse_threads, default=[1, 4], help="Rayon thread counts (default: 1,4)")
    parser.add_argument("--cpus", type=cpu_list, help="CPU affinity (default: first N available CPUs, N=max threads)")
    parser.add_argument("--samples", type=positive_integer, default=100, help="samples per case and process (default: 100)")
    parser.add_argument("--rounds", type=positive_integer, default=2, help="ABBA rounds per thread count (default: 2)")
    parser.add_argument("--filter", default="", help="case substring passed as MPT_BENCH_FILTER")
    args = parser.parse_args()
    if not shutil.which("taskset") or not hasattr(os, "sched_getaffinity"):
        parser.error("this runner requires Linux CPU affinity and taskset")
    available = os.sched_getaffinity(0)
    cpus = args.cpus or sorted(available)[:max(args.threads)]
    if not set(cpus) <= available:
        parser.error(f"requested CPUs are outside available affinity: {sorted(available)}")
    if max(args.threads) > len(cpus):
        parser.error("provide at least as many CPUs as the largest thread count")
    if args.output.exists():
        parser.error(f"output directory already exists: {args.output}")
    args.output.mkdir(parents=True)
    runs_directory = args.output / "runs"
    runs_directory.mkdir()
    binaries = {"baseline": args.baseline, "candidate": args.candidate}
    metadata = {
        "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platform": platform.platform(),
        "cpu_affinity": cpus,
        "rayon_threads": args.threads,
        "samples_per_case": args.samples,
        "rounds": args.rounds,
        "order_per_round": ORDER,
        "filter": args.filter,
        "binaries": {mode: {"path": str(path), "sha256": digest(path)} for mode, path in binaries.items()},
        "summary_statistics": "median and inclusive quartiles of process-level sample medians",
    }
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.exists():
        metadata["cpu_model"] = next(
            (line.split(":", 1)[1].strip() for line in cpuinfo.read_text().splitlines()
             if line.startswith("model name")), "unknown"
        )
    metadata_path = args.output / "metadata.json"
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
    records = []
    expected_roots = None
    for threads in args.threads:
        for round_index in range(args.rounds):
            for position, mode in enumerate(ORDER):
                name = f"threads-{threads}-round-{round_index + 1}-run-{position + 1}-{mode}"
                print(f"Running {name} ({args.samples} samples per case)", flush=True)
                environment = os.environ.copy()
                environment.update(
                    RAYON_NUM_THREADS=str(threads), MPT_BENCH_SAMPLES=str(args.samples),
                    MPT_BENCH_FILTER=args.filter,
                )
                command = ["taskset", "--cpu-list", ",".join(map(str, cpus)), str(binaries[mode])]
                result = subprocess.run(command, env=environment, text=True, capture_output=True, check=False)
                (runs_directory / f"{name}.csv").write_text(result.stdout)
                if result.stderr:
                    (runs_directory / f"{name}.stderr").write_text(result.stderr)
                if result.returncode:
                    raise RuntimeError(f"{name} exited {result.returncode}; inspect {runs_directory}")
                rows = parse_result(result.stdout)
                roots = {case: row["root"] for case, row in rows.items()}
                if expected_roots is None:
                    expected_roots = roots
                elif roots != expected_roots:
                    raise ValueError(f"case set or root mismatch in {name}")
                if any(row["samples"] != args.samples for row in rows.values()):
                    raise ValueError(f"unexpected sample count in {name}")
                records.extend(
                    dict(row, threads=threads, round=round_index + 1, position=position + 1, mode=mode)
                    for row in rows.values()
                )
    with (args.output / "runs.csv").open("w", newline="") as destination:
        writer = csv.DictWriter(destination, fieldnames=["threads", "round", "position", "mode", *FIELDS])
        writer.writeheader()
        writer.writerows(records)
    summarize(records, args.output / "summary.csv")
    metadata["finished_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
    print(f"Wrote {args.output / 'summary.csv'}", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError) as error:
        sys.exit(str(error))
