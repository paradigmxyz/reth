#!/usr/bin/env python3
"""Summarize an RPC replay benchmark and generate summary JSON + markdown.

Usage:
    bench-call-summary.py \
        --output-summary <summary.json> \
        --output-markdown <comment.md> \
        --baseline-dir <baseline-1> [<baseline-2> ...] \
        --feature-dir <feature-1> [<feature-2> ...]

Each run directory holds the output of one `bench call` measured phase:

    report.json          run metadata, identity and per-method aggregates
    requests.csv         open-loop, one row per request
    record_timings.csv   closed-loop, one row per record and pass
    responses.ndjson     one response digest per corpus record
    cpu.json             CPU time the node's cgroup spent on the measured phase

Latency, throughput and CPU per request are compared with whole-run cluster
bootstrap confidence intervals, and every response digest is compared against
the first baseline run. Any digest that differs between the arms fails the
verdict regardless of the timings; digests that already differ between two
baseline runs make the comparison inconclusive instead.
"""

from __future__ import annotations

import argparse
import csv
import importlib.util
import json
import os
from pathlib import Path
import random
import sys

# Latency floors below which a delta is treated as noise. These are starting
# values for the first runs on the bench runner and are meant to be tightened
# once the A/A spread of each class is known.
PRACTICAL_FLOOR_PCT = {
    "mean": 2.5,
    "p50": 2.5,
    "p90": 2.5,
    "p99": 5.0,
    "record_median": 2.5,
    "closed_loop_rps": 2.5,
    "cpu_per_request": 2.5,
}
MAX_DIVERGENT_REPORTED = 40
OK_STATUS = "ok"


def _load_engine_summary() -> object:
    """Import the engine summary script for its statistics helpers."""
    path = Path(__file__).resolve().with_name("bench-reth-summary.py")
    spec = importlib.util.spec_from_file_location("bench_reth_summary", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_engine = _load_engine_summary()
BOOTSTRAP_ITERATIONS = _engine.BOOTSTRAP_ITERATIONS
_bootstrap_ci = _engine._bootstrap_ci
_ci_half_width = _engine._ci_half_width
_mean = _engine._mean
build_grafana_logs_url = _engine.build_grafana_logs_url
build_grafana_traces_url = _engine.build_grafana_traces_url
fmt_ms = _engine.fmt_ms
generate_observability_section = _engine.generate_observability_section
percentile = _engine.percentile
resolve_observability_range = _engine.resolve_observability_range
significance = _engine.significance


def load_run(directory: Path) -> dict:
    """Load one run directory into the per-run shape used by the comparison."""
    report_path = directory / "report.json"
    if not report_path.is_file():
        raise SystemExit(f"Missing {report_path}")
    with report_path.open() as f:
        report = json.load(f)

    responses_path = directory / "responses.ndjson"
    if not responses_path.is_file():
        raise SystemExit(f"Missing {responses_path}")

    requests = parse_requests_csv(directory / "requests.csv")
    records = parse_record_timings_csv(directory / "record_timings.csv")
    if not requests and not records:
        raise SystemExit(f"No open-loop or closed-loop timings in {directory}")

    cpu = {}
    cpu_path = directory / "cpu.json"
    if cpu_path.is_file():
        with cpu_path.open() as f:
            cpu = json.load(f)

    return {
        "name": directory.name,
        "report": report,
        "requests": requests,
        "records": records,
        "responses": parse_responses(responses_path),
        "cpu": cpu,
    }


def parse_requests_csv(path: Path) -> list[dict]:
    """Parse the open-loop per-request CSV."""
    if not path.is_file():
        return []
    rows = []
    with path.open() as f:
        for row in csv.DictReader(f):
            rows.append(
                {
                    "offset_ms": float(row["offset_ms"]),
                    "record_index": int(row["record_index"]),
                    "method": row["method"],
                    "latency_us": int(row["latency_us"]),
                    "status": row["status"],
                }
            )
    return rows


def parse_record_timings_csv(path: Path) -> list[dict]:
    """Parse the closed-loop per-record CSV."""
    if not path.is_file():
        return []
    rows = []
    with path.open() as f:
        for row in csv.DictReader(f):
            rows.append(
                {
                    "record_index": int(row["record_index"]),
                    "method": row["method"],
                    "pass": int(row["pass"]),
                    "latency_us": int(row["latency_us"]),
                    "status": row["status"],
                }
            )
    return rows


def parse_responses(path: Path) -> dict[int, dict]:
    """Parse response digests, keyed by record index."""
    responses = {}
    with path.open() as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            entry = json.loads(line)
            responses[int(entry["record_index"])] = {
                "method": entry.get("method", "unknown"),
                "kind": entry.get("kind", "unknown"),
                "digest": entry.get("digest", ""),
                "len": entry.get("len"),
            }
    return responses


def call_section(report: dict) -> dict:
    """The replay section of a report; `bench call` nests its fields under `call`."""
    section = report.get("call")
    return section if isinstance(section, dict) else report


def report_field(report: dict, key: str):
    """Look a field up in the report, tolerating the nested `call` and identity blocks."""
    for scope in (report, call_section(report)):
        if key in scope:
            return scope[key]
        for nested in ("identity", "metadata", "meta"):
            block = scope.get(nested)
            if isinstance(block, dict) and key in block:
                return block[key]
    return None


def median(values: list[float]) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    mid = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[mid]
    return (ordered[mid - 1] + ordered[mid]) / 2.0


def open_loop_stats(rows: list[dict]) -> dict:
    """Latency distribution, achieved rate and error rate of one open-loop run."""
    if not rows:
        return {}
    ok_rows = [row for row in rows if row["status"] == OK_STATUS]
    latencies_ms = sorted(row["latency_us"] / 1_000 for row in ok_rows)
    offsets = [row["offset_ms"] for row in rows]
    span_s = (max(offsets) - min(offsets)) / 1_000 if len(rows) > 1 else 0.0
    return {
        "requests": len(rows),
        "ok": len(ok_rows),
        "errors": len(rows) - len(ok_rows),
        "error_rate_pct": (len(rows) - len(ok_rows)) / len(rows) * 100.0,
        "mean_ms": _mean(latencies_ms),
        "p50_ms": percentile(latencies_ms, 50),
        "p90_ms": percentile(latencies_ms, 90),
        "p99_ms": percentile(latencies_ms, 99),
        "open_loop_rps": len(rows) / span_s if span_s > 0 else 0.0,
    }


def record_medians(rows: list[dict]) -> dict[int, float]:
    """Median closed-loop latency in ms per record, over the passes of one run."""
    by_record: dict[int, list[float]] = {}
    for row in rows:
        if row["status"] != OK_STATUS:
            continue
        by_record.setdefault(row["record_index"], []).append(row["latency_us"] / 1_000)
    return {index: median(values) for index, values in by_record.items()}


def arm_record_medians(runs: list[dict], method: str | None = None) -> dict[int, float]:
    """Median closed-loop latency per record, over all passes and runs of one arm."""
    by_record: dict[int, list[float]] = {}
    for run in runs:
        for row in run["records"]:
            if row["status"] != OK_STATUS:
                continue
            if method is not None and row["method"] != method:
                continue
            by_record.setdefault(row["record_index"], []).append(row["latency_us"] / 1_000)
    return {index: median(values) for index, values in by_record.items()}


def run_stats(run: dict, method: str | None = None) -> dict:
    """Every displayed metric for one run, optionally restricted to one method."""
    requests = run["requests"]
    records = run["records"]
    if method is not None:
        requests = [row for row in requests if row["method"] == method]
        records = [row for row in records if row["method"] == method]

    stats = open_loop_stats(requests)
    medians = record_medians(records)
    if medians:
        stats["record_median_ms"] = median(list(medians.values()))
        stats["records"] = len(medians)

    report = run["report"]
    if method is None:
        closed_loop_rps = report_field(report, "closed_loop_rps")
        dropped = report_field(report, "dropped")
    else:
        methods = call_section(report).get("methods") or {}
        entry = methods.get(method) or {}
        closed_loop_rps = entry.get("closed_loop_rps")
        if closed_loop_rps is None and isinstance(entry.get("closed_loop"), dict):
            closed_loop_rps = entry["closed_loop"].get("rps")
        dropped = entry.get("dropped")
    if isinstance(closed_loop_rps, (int, float)):
        stats["closed_loop_rps"] = float(closed_loop_rps)
    if isinstance(dropped, int):
        stats["dropped"] = dropped

    if method is None:
        cpu_ms = cpu_ms_per_request(run)
        if cpu_ms is not None:
            stats["cpu_ms_per_request"] = cpu_ms

    lengths = [
        entry["len"]
        for entry in run["responses"].values()
        if isinstance(entry.get("len"), int) and (method is None or entry["method"] == method)
    ]
    if lengths:
        stats["response_bytes_median"] = median([float(v) for v in lengths])
    return stats


def cpu_ms_per_request(run: dict) -> float | None:
    """CPU milliseconds the node spent per successful request of one run."""
    cpu = run["cpu"]
    usage_usec = cpu.get("usage_usec_delta")
    if not isinstance(usage_usec, (int, float)) or usage_usec <= 0:
        return None
    requests_ok = cpu.get("requests_ok")
    if not isinstance(requests_ok, int) or requests_ok <= 0:
        requests_ok = sum(
            1 for row in run["requests"] + run["records"] if row["status"] == OK_STATUS
        )
    if requests_ok <= 0:
        return None
    return usage_usec / 1_000 / requests_ok


def arm_stats(runs: list[dict], method: str | None = None) -> dict:
    """Point estimates for one arm, averaged over its runs."""
    per_run = [run_stats(run, method) for run in runs]
    per_run = [stats for stats in per_run if stats]
    if not per_run:
        return {}

    stats = {"runs": len(per_run)}
    for key in (
        "mean_ms",
        "p50_ms",
        "p90_ms",
        "p99_ms",
        "open_loop_rps",
        "closed_loop_rps",
        "record_median_ms",
        "cpu_ms_per_request",
        "error_rate_pct",
        "response_bytes_median",
    ):
        values = [run[key] for run in per_run if key in run]
        if values:
            stats[key] = _mean(values)
    for key in ("requests", "ok", "errors", "dropped"):
        values = [run[key] for run in per_run if key in run]
        if values:
            stats[key] = sum(values)
    records = [run["records"] for run in per_run if "records" in run]
    if records:
        stats["records"] = max(records)
    return stats


def per_run_values(runs: list[dict], method: str | None = None) -> dict[str, list[float]]:
    """One value per run and metric, the input of the cluster bootstrap."""
    values: dict[str, list[float]] = {}
    for run in runs:
        stats = run_stats(run, method)
        for key, value in stats.items():
            if isinstance(value, (int, float)):
                values.setdefault(key, []).append(float(value))
    return values


def cluster_bootstrap_ci(
    rng: random.Random,
    baseline_values: dict[str, list[float]],
    feature_values: dict[str, list[float]],
    metrics: tuple[str, ...],
    n_iter: int = BOOTSTRAP_ITERATIONS,
) -> dict[str, float]:
    """Resample whole runs to estimate run-to-run noise per metric.

    Mirrors the engine summary's cluster bootstrap, which resamples block rows
    and cannot be reused for request shaped runs.
    """
    empty = {metric: 0.0 for metric in metrics}
    usable = [
        metric
        for metric in metrics
        if len(baseline_values.get(metric, [])) >= 2 and len(feature_values.get(metric, [])) >= 2
    ]
    if not usable:
        return empty

    samples: dict[str, list[float]] = {metric: [] for metric in usable}
    for metric in usable:
        baseline_metric = baseline_values[metric]
        feature_metric = feature_values[metric]
        baseline_count = len(baseline_metric)
        feature_count = len(feature_metric)
        for _ in range(n_iter):
            baseline_sample = [baseline_metric[rng.randrange(baseline_count)] for _ in range(baseline_count)]
            feature_sample = [feature_metric[rng.randrange(feature_count)] for _ in range(feature_count)]
            samples[metric].append(_mean(feature_sample) - _mean(baseline_sample))

    result = dict(empty)
    for metric in usable:
        result[metric] = _ci_half_width(samples[metric])
    return result


def paired_record_delta(
    rng: random.Random,
    baseline_runs: list[dict],
    feature_runs: list[dict],
    method: str | None = None,
) -> dict | None:
    """Per-record closed-loop delta with a bootstrap CI over records."""
    baseline_medians = arm_record_medians(baseline_runs, method)
    feature_medians = arm_record_medians(feature_runs, method)
    common = sorted(set(baseline_medians) & set(feature_medians))
    if not common:
        return None
    diffs = [feature_medians[index] - baseline_medians[index] for index in common]
    base = _mean([baseline_medians[index] for index in common])
    return {
        "records": len(common),
        "baseline_ms": base,
        "feature_ms": _mean([feature_medians[index] for index in common]),
        "delta_ms": _mean(diffs),
        "ci_ms": _bootstrap_ci(rng, diffs),
    }


def make_change(
    baseline_value: float | None,
    feature_value: float | None,
    ci_value: float,
    floor_pct: float,
    lower_is_better: bool,
    aa_pct: float | None = None,
) -> dict | None:
    """Build one `changes` entry in the shape the comment renderers consume."""
    if baseline_value is None or feature_value is None or baseline_value <= 0:
        return None
    pct = (feature_value - baseline_value) / baseline_value * 100.0
    ci_pct = ci_value / baseline_value * 100.0 if ci_value else 0.0
    change = {
        "pct": round(pct, 4),
        "ci_pct": round(ci_pct, 4),
        "floor_pct": round(floor_pct, 4),
        "sig": significance(pct, ci_pct, floor_pct, lower_is_better),
        "baseline": baseline_value,
        "feature": feature_value,
    }
    if aa_pct is not None:
        change["aa_pct"] = round(aa_pct, 4)
    return change


def aa_spread(runs: list[dict], method: str | None = None) -> dict[str, float]:
    """Percent difference between the first two baseline runs, the noise floor."""
    if len(runs) < 2:
        return {}
    first = run_stats(runs[0], method)
    second = run_stats(runs[1], method)
    spread = {}
    for key, value in first.items():
        if not isinstance(value, (int, float)) or value <= 0:
            continue
        other = second.get(key)
        if isinstance(other, (int, float)):
            spread[key] = (other - value) / value * 100.0
    return spread


def compute_changes(
    rng: random.Random,
    baseline_runs: list[dict],
    feature_runs: list[dict],
    baseline_stats: dict,
    feature_stats: dict,
    method: str | None = None,
) -> dict:
    """Compare one corpus or one method between the two arms."""
    metrics = (
        ("mean", "mean_ms", True),
        ("p50", "p50_ms", True),
        ("p90", "p90_ms", True),
        ("p99", "p99_ms", True),
        ("closed_loop_rps", "closed_loop_rps", False),
        ("cpu_per_request", "cpu_ms_per_request", True),
    )
    ci = cluster_bootstrap_ci(
        rng,
        per_run_values(baseline_runs, method),
        per_run_values(feature_runs, method),
        tuple(stat_key for _, stat_key, _ in metrics) + ("record_median_ms",),
    )
    spread = aa_spread(baseline_runs, method)

    changes = {}
    for name, stat_key, lower_is_better in metrics:
        change = make_change(
            baseline_stats.get(stat_key),
            feature_stats.get(stat_key),
            ci.get(stat_key, 0.0),
            PRACTICAL_FLOOR_PCT.get(name, 0.0),
            lower_is_better,
            spread.get(stat_key),
        )
        if change:
            changes[name] = change

    # The point estimate is the per-run record median averaged over runs, so its
    # interval comes from the same whole-run bootstrap as the other metrics; the
    # per-record pairing is kept as supplementary detail only.
    change = make_change(
        baseline_stats.get("record_median_ms"),
        feature_stats.get("record_median_ms"),
        ci.get("record_median_ms", 0.0),
        PRACTICAL_FLOOR_PCT["record_median"],
        True,
        spread.get("record_median_ms"),
    )
    if change:
        paired = paired_record_delta(rng, baseline_runs, feature_runs, method)
        if paired:
            change["records"] = paired["records"]
            change["paired_delta_ms"] = round(paired["delta_ms"], 6)
        changes["record_median"] = change
    return changes


def parity_entry() -> dict:
    return {
        "matched": 0,
        "content_mismatch": 0,
        "kind_mismatch": 0,
        "missing": 0,
        "nondeterministic": 0,
        "excluded": 0,
        "divergent_records": [],
    }


def compare_responses(
    reference: dict[int, dict],
    other: dict[int, dict],
    exclude: frozenset[int] = frozenset(),
) -> dict:
    """Compare one run's digests against the reference run, per method.

    Records in `exclude` are unstable in the baseline itself and are only counted,
    never judged.
    """
    per_method: dict[str, dict] = {}
    for index, expected in reference.items():
        method = expected["method"]
        entry = per_method.setdefault(method, parity_entry())
        if index in exclude:
            entry["excluded"] += 1
            continue
        actual = other.get(index)
        if actual is None:
            entry["missing"] += 1
        elif actual["kind"] != expected["kind"]:
            entry["kind_mismatch"] += 1
        elif actual["digest"] != expected["digest"]:
            entry["content_mismatch"] += 1
        else:
            entry["matched"] += 1
            continue
        if len(entry["divergent_records"]) < MAX_DIVERGENT_REPORTED:
            entry["divergent_records"].append(index)
    # A record the reference run never answered but this run did is just as divergent.
    for index, actual in other.items():
        if index in reference:
            continue
        entry = per_method.setdefault(actual["method"], parity_entry())
        if index in exclude:
            entry["excluded"] += 1
            continue
        entry["missing"] += 1
        if len(entry["divergent_records"]) < MAX_DIVERGENT_REPORTED:
            entry["divergent_records"].append(index)
    return per_method


def merge_parity(target: dict, addition: dict) -> None:
    for method, counts in addition.items():
        entry = target.setdefault(method, parity_entry())
        for key in ("matched", "content_mismatch", "kind_mismatch", "missing", "nondeterministic", "excluded"):
            entry[key] += counts.get(key, 0)
        for index in counts["divergent_records"]:
            if index not in entry["divergent_records"] and len(entry["divergent_records"]) < MAX_DIVERGENT_REPORTED:
                entry["divergent_records"].append(index)


def parity_totals(per_method: dict) -> dict:
    totals = {
        "matched": 0,
        "content_mismatch": 0,
        "kind_mismatch": 0,
        "missing": 0,
        "nondeterministic": 0,
        "excluded": 0,
    }
    for counts in per_method.values():
        for key in totals:
            totals[key] += counts.get(key, 0)
    # A record answered in only one of the two runs, or answered differently
    # within the feature runs alone, counts as a divergence.
    totals["mismatched"] = (
        totals["content_mismatch"] + totals["kind_mismatch"] + totals["missing"] + totals["nondeterministic"]
    )
    return totals


def nondeterministic_count(runs: list[dict]) -> int:
    total = 0
    for run in runs:
        entries = report_field(run["report"], "nondeterministic")
        if isinstance(entries, list):
            total += len(entries)
        elif isinstance(entries, int):
            total += entries
    return total


def nondeterministic_records(runs: list[dict]) -> tuple[set[int], bool]:
    """Record indexes a run answered differently within itself.

    The second value is true when a run only reported a count, so the records
    cannot be identified.
    """
    indexes: set[int] = set()
    unknown = False
    for run in runs:
        entries = report_field(run["report"], "nondeterministic")
        if isinstance(entries, list):
            indexes.update(int(index) for index in entries)
            total = report_field(run["report"], "nondeterministic_total")
            if isinstance(total, int) and total > len(entries):
                unknown = True
        elif isinstance(entries, int) and entries > 0:
            unknown = True
    return indexes, unknown


def compute_parity(baseline_runs: list[dict], feature_runs: list[dict]) -> dict:
    """Compare every run's digests against the first baseline run.

    Records that are unstable in the baseline itself, answered differently
    between baseline runs or within one, are excluded from the decision and
    reported; the feature is judged on the remaining stable records, where its
    own nondeterminism counts as a divergence.
    """
    reference = baseline_runs[0]["responses"]

    baseline_parity: dict[str, dict] = {}
    for run in baseline_runs[1:]:
        merge_parity(baseline_parity, compare_responses(reference, run["responses"]))
    baseline_nondeterministic, baseline_unknown = nondeterministic_records(baseline_runs)
    unstable = set(baseline_nondeterministic)
    for counts in baseline_parity.values():
        unstable.update(counts["divergent_records"])
    baseline_totals = parity_totals(baseline_parity)
    if baseline_totals["mismatched"] > sum(len(c["divergent_records"]) for c in baseline_parity.values()):
        # More baseline divergences than we could list: the unstable set is incomplete.
        baseline_unknown = True

    feature_nondeterministic, feature_unknown = nondeterministic_records(feature_runs)
    excluded = frozenset(unstable)
    feature_parity: dict[str, dict] = {}
    for run in feature_runs:
        merge_parity(feature_parity, compare_responses(reference, run["responses"], excluded))
    for index in sorted(feature_nondeterministic - unstable):
        expected = reference.get(index)
        if expected is None:
            continue
        entry = feature_parity.setdefault(expected["method"], parity_entry())
        entry["nondeterministic"] += 1
        if index not in entry["divergent_records"] and len(entry["divergent_records"]) < MAX_DIVERGENT_REPORTED:
            entry["divergent_records"].append(index)
    feature_totals = parity_totals(feature_parity)
    nondeterministic = nondeterministic_count(baseline_runs + feature_runs)

    records = len(reference)
    stable_records = records - len(unstable & set(reference))
    if baseline_unknown or feature_unknown or (unstable and stable_records == 0):
        status = "inconclusive"
    elif feature_totals["mismatched"]:
        status = "mismatch"
    else:
        status = "matched"

    excluded_list = sorted(unstable)
    return {
        "status": status,
        "records": records,
        "feature": {"totals": feature_totals, "methods": feature_parity},
        "baseline": {"totals": baseline_totals, "methods": baseline_parity},
        "nondeterministic": nondeterministic,
        "excluded": excluded_list[:MAX_DIVERGENT_REPORTED],
        "excluded_count": len(excluded_list),
        "line": parity_line(
            status, records, feature_totals, baseline_totals, nondeterministic, feature_parity, excluded_list
        ),
    }


def parity_line(
    status: str,
    records: int,
    feature_totals: dict,
    baseline_totals: dict,
    nondeterministic: int,
    feature_parity: dict,
    excluded: list[int] | None = None,
) -> str:
    excluded = excluded or []
    excluded_note = ""
    if excluded:
        shown = ", ".join(str(index) for index in excluded[:MAX_DIVERGENT_REPORTED])
        excluded_note = f"; {len(excluded)} unstable in the baseline, excluded (records {shown})"
    checked = feature_totals["matched"] + feature_totals["mismatched"]
    if status == "matched":
        return (
            f"✅ matched {feature_totals['matched']}/{checked} responses "
            f"over {records} records{excluded_note}"
        )

    details = []
    for method, counts in sorted(feature_parity.items()):
        mismatched = (
            counts["content_mismatch"] + counts["kind_mismatch"] + counts["missing"] + counts.get("nondeterministic", 0)
        )
        if not mismatched:
            continue
        indexes = ", ".join(str(index) for index in counts["divergent_records"])
        details.append(f"`{method}`: {mismatched} (records {indexes})")

    if status == "mismatch":
        return "❌ {} content, {} kind mismatches, {} answered in one arm only, {} nondeterministic in the feature only — {}{}".format(
            feature_totals["content_mismatch"],
            feature_totals["kind_mismatch"],
            feature_totals["missing"],
            feature_totals.get("nondeterministic", 0),
            "; ".join(details) if details else "no per-method detail",
            excluded_note,
        )

    reasons = []
    if baseline_totals["mismatched"]:
        reasons.append(f"{baseline_totals['mismatched']} baseline-vs-baseline mismatches")
    if nondeterministic:
        reasons.append(f"{nondeterministic} records answered differently within a run")
    if not reasons:
        reasons.append("no stable records to judge")
    suffix = f"; feature mismatches: {feature_totals['mismatched']}" if feature_totals["mismatched"] else ""
    return "⚠️ inconclusive — " + ", ".join(reasons) + suffix


def parity_change(parity: dict) -> dict:
    """Fold parity into `changes` so the shared verdict helper sees it."""
    totals = parity["feature"]["totals"]
    change = {
        "pct": 0.0,
        "ci_pct": 0.0,
        "floor_pct": 0.0,
        "sig": "bad" if parity["status"] == "mismatch" else "neutral",
        "mismatched": totals["mismatched"],
        "excluded": parity.get("excluded_count", 0),
    }
    if parity["status"] == "inconclusive":
        change["informational"] = True
        change["informational_reason"] = "informational, responses are not deterministic"
    return change


def corpus_methods(baseline_runs: list[dict], feature_runs: list[dict]) -> list[str]:
    methods = set()
    for run in baseline_runs + feature_runs:
        for entry in run["responses"].values():
            methods.add(entry["method"])
        for row in run["requests"]:
            methods.add(row["method"])
    return sorted(methods)


def assert_same_head(runs: list[dict]) -> str | None:
    """Refuse to compare runs that replayed against different chain tips."""
    hashes = {}
    for run in runs:
        head_hash = report_field(run["report"], "head_hash")
        if head_hash:
            hashes[run["name"]] = head_hash
    distinct = set(hashes.values())
    if len(distinct) > 1:
        detail = ", ".join(f"{name}={value}" for name, value in sorted(hashes.items()))
        raise SystemExit(f"Runs replayed against different chain tips: {detail}")
    return next(iter(distinct), None)


def fmt_opt_ms(value: float | None) -> str:
    return fmt_ms(value) if isinstance(value, (int, float)) else "n/a"


def fmt_opt_num(value: float | None, digits: int = 2) -> str:
    return f"{value:.{digits}f}" if isinstance(value, (int, float)) else "n/a"


def change_str(change: dict | None) -> str:
    if not change:
        return "n/a"
    emoji = {"good": "✅", "bad": "❌", "neutral": "⚪"}[
        "neutral" if change.get("informational") else change["sig"]
    ]
    details = [f"±{change['ci_pct']:.2f}%", f"floor {change['floor_pct']:.2f}%"]
    if "aa_pct" in change:
        details.append(f"A/A {change['aa_pct']:+.2f}%")
    if change.get("informational"):
        details.append("informational")
    return f"{change['pct']:+.2f}% {emoji} ({', '.join(details)})"


def generate_metric_table(summary: dict, baseline_label: str, feature_label: str) -> str:
    baseline = summary["baseline"]["stats"]
    feature = summary["feature"]["stats"]
    changes = summary["changes"]
    rows = [
        ("Mean", "mean_ms", "mean", fmt_opt_ms),
        ("P50", "p50_ms", "p50", fmt_opt_ms),
        ("P90", "p90_ms", "p90", fmt_opt_ms),
        ("P99", "p99_ms", "p99", fmt_opt_ms),
        ("Record median", "record_median_ms", "record_median", fmt_opt_ms),
        ("Closed-loop rps", "closed_loop_rps", "closed_loop_rps", fmt_opt_num),
        ("CPU / request", "cpu_ms_per_request", "cpu_per_request", fmt_opt_ms),
    ]
    lines = [
        f"| Metric | {baseline_label} | {feature_label} | Change |",
        "|--------|------|--------|--------|",
    ]
    for label, stat_key, change_key, formatter in rows:
        if stat_key not in baseline and stat_key not in feature:
            continue
        lines.append(
            f"| {label} | {formatter(baseline.get(stat_key))} | "
            f"{formatter(feature.get(stat_key))} | {change_str(changes.get(change_key))} |"
        )
    lines.append(
        f"| Error rate | {fmt_opt_num(baseline.get('error_rate_pct'))}% | "
        f"{fmt_opt_num(feature.get('error_rate_pct'))}% | |"
    )
    lines.append(
        f"| Open-loop rps | {fmt_opt_num(baseline.get('open_loop_rps'))} | "
        f"{fmt_opt_num(feature.get('open_loop_rps'))} | |"
    )
    lines.append("")
    return "\n".join(lines)


def generate_method_table(summary: dict) -> str:
    methods = summary.get("methods") or {}
    if len(methods) < 2:
        return ""
    lines = [
        "### Per-method",
        "",
        "| Method | Records | Baseline mean | Feature mean | Change | Response bytes | Parity |",
        "|--------|---------|---------------|--------------|--------|----------------|--------|",
    ]
    for method in sorted(methods):
        entry = methods[method]
        baseline = entry["baseline"]
        feature = entry["feature"]
        totals = entry["parity"]
        mismatched = totals["content_mismatch"] + totals["kind_mismatch"]
        parity = f"{totals['matched']} matched" if not mismatched else f"❌ {mismatched} mismatched"
        lines.append(
            "| `{}` | {} | {} | {} | {} | {} | {} |".format(
                method,
                baseline.get("records", baseline.get("requests", 0)),
                fmt_opt_ms(baseline.get("mean_ms")),
                fmt_opt_ms(feature.get("mean_ms")),
                change_str(entry["changes"].get("mean")),
                fmt_opt_num(baseline.get("response_bytes_median"), 0),
                parity,
            )
        )
    lines.append("")
    return "\n".join(lines)


def generate_markdown(
    summary: dict,
    baseline_label: str,
    feature_label: str,
    behind_baseline: int = 0,
    repo: str = "",
    baseline_ref: str = "",
    baseline_name: str = "",
    derek_command: str | None = None,
) -> str:
    corpus = summary["corpus"]
    lines = ["## Benchmark Results", "", "## Configuration"]
    if derek_command:
        lines.append(f"- Derek command: `{derek_command}`")
    methods = ", ".join(corpus.get("methods") or summary.get("replayed_methods") or []) or "all"
    top_gas_note = f" (top-gas {corpus['top_gas']})" if corpus.get("top_gas") else ""
    tracer_notes = []
    if corpus.get("tracer"):
        tracer_notes.append("tracer " + ", ".join(corpus["tracer"]))
    for key in ("tracer_config", "trace_options"):
        if corpus.get(key):
            tracer_notes.append(f"{key.replace('_', '-')} `{json.dumps(corpus[key], separators=(',', ':'))}`")
    tracer_note = f", {'; '.join(tracer_notes)}" if tracer_notes else ""
    blocks_note = ""
    if corpus.get("blocks_source"):
        blocks_note = f", blocks {corpus.get('corpus_from')}..{corpus.get('corpus_to')} from {corpus['blocks_source']}"
        if corpus.get("block_param") == "parent":
            blocks_note += " at parent-block state"
    lines.append(
        f"- Corpus: `{corpus.get('name', 'static')}` ({corpus.get('source', 'static')}), "
        f"class `{corpus.get('class', 'call')}`{top_gas_note}{tracer_note}{blocks_note}, {corpus.get('records', summary['records'])} records, "
        f"methods: {methods}"
    )
    lines.append(
        f"- Replay: {summary['rps']} rps for {summary['duration']}, "
        f"{summary['passes']} passes x {summary['concurrency']} workers, "
        f"{summary['warmup_seconds']}s warmup, "
        f"max tracing requests: {summary.get('max_tracing_requests') or 'default'}"
    )
    if corpus.get("tip"):
        lines.append(f"- Tip: {corpus['tip']} (`{corpus.get('tip_hash', 'unknown')}`)")
    lines.append("")
    if behind_baseline > 0:
        plural = "s" if behind_baseline > 1 else ""
        diff_link = f"https://github.com/{repo}/compare/{baseline_ref[:12]}...{baseline_name}"
        lines.append(
            f"> ⚠️ Feature is [**{behind_baseline} commit{plural} behind `{baseline_name}`**]({diff_link}). "
            "Consider rebasing for accurate results."
        )
        lines.append("")
    lines.append(generate_metric_table(summary, baseline_label, feature_label))
    meta_parts = [
        f"{summary['records']} records",
        "mode: call",
        f"{summary['run_pairs']} run pairs" if summary.get("run_pairs") else "",
    ]
    lines.append(f"*{', '.join(part for part in meta_parts if part)}*")
    lines.append("")
    lines.append(f"**Parity:** {summary['parity']['line']}")
    method_table = generate_method_table(summary)
    if method_table:
        lines.append("")
        lines.append(method_table)
    lines.extend(generate_observability_section(summary))
    return "\n".join(lines)


def build_summary(args, baseline_runs: list[dict], feature_runs: list[dict]) -> dict:
    rng = random.Random(42)
    head_hash = assert_same_head(baseline_runs + feature_runs)

    baseline_stats = arm_stats(baseline_runs)
    feature_stats = arm_stats(feature_runs)
    changes = compute_changes(rng, baseline_runs, feature_runs, baseline_stats, feature_stats)
    parity = compute_parity(baseline_runs, feature_runs)
    changes["parity"] = parity_change(parity)

    corpus = {"name": "static", "source": "static", "class": args.klass}
    if args.corpus_meta:
        meta_path = Path(args.corpus_meta)
        if meta_path.is_file():
            with meta_path.open() as f:
                corpus.update(json.load(f))
    if head_hash and not corpus.get("tip_hash"):
        corpus["tip_hash"] = head_hash

    methods = corpus_methods(baseline_runs, feature_runs)
    method_entries = {}
    if len(methods) > 1:
        for method in methods:
            method_baseline = arm_stats(baseline_runs, method)
            method_feature = arm_stats(feature_runs, method)
            if not method_baseline or not method_feature:
                continue
            method_entries[method] = {
                "baseline": method_baseline,
                "feature": method_feature,
                "changes": compute_changes(
                    rng, baseline_runs, feature_runs, method_baseline, method_feature, method
                ),
                "parity": parity["feature"]["methods"].get(
                    method,
                    {"matched": 0, "content_mismatch": 0, "kind_mismatch": 0, "missing": 0, "divergent_records": []},
                ),
            }

    records = parity["records"] or corpus.get("records") or 0
    summary = {
        "mode": "call",
        "blocks": records,
        "records": records,
        "corpus": corpus,
        "replayed_methods": methods,
        "rps": args.rps,
        "duration": args.duration,
        "passes": args.passes,
        "concurrency": args.concurrency,
        "warmup_seconds": args.warmup_seconds,
        "run_pairs": args.run_pairs,
        "baseline": {
            "name": args.baseline_name or "baseline",
            "ref": args.baseline_ref or "main",
            "stats": baseline_stats,
        },
        "feature": {
            "name": args.feature_name or "feature",
            "ref": args.feature_ref or "unknown",
            "stats": feature_stats,
        },
        "changes": changes,
        "methods": method_entries,
        "parity": parity,
    }
    if args.max_tracing_requests:
        summary["max_tracing_requests"] = args.max_tracing_requests
    if args.benchmark_id:
        summary["benchmark_id"] = args.benchmark_id

    from_ms, to_ms = resolve_observability_range(
        args.observability_from_ms, args.observability_to_ms
    )
    logs_url = args.logs_url
    traces_url = args.traces_url
    if args.benchmark_id and from_ms and to_ms:
        logs_url = logs_url or build_grafana_logs_url(args.benchmark_id, from_ms, to_ms)
        traces_url = traces_url or build_grafana_traces_url(args.benchmark_id, from_ms, to_ms)
    observability = {
        key: value
        for key, value in {
            "benchmark_id": args.benchmark_id,
            "from_ms": from_ms,
            "to_ms": to_ms,
            "grafana_url": args.grafana_url,
            "logs_url": logs_url,
            "traces_url": traces_url,
        }.items()
        if value
    }
    if observability:
        summary["observability"] = observability
    return summary


def parse_args(argv: list[str] | None = None):
    parser = argparse.ArgumentParser(description="Summarize an RPC replay benchmark")
    parser.add_argument("--baseline-dir", nargs="+", required=True, help="Baseline run directories")
    parser.add_argument("--feature-dir", nargs="+", required=True, help="Feature run directories")
    parser.add_argument("--output-summary", required=True, help="Output JSON summary path")
    parser.add_argument("--output-markdown", required=True, help="Output markdown path")
    parser.add_argument("--repo", default="paradigmxyz/reth", help="GitHub repo (owner/name)")
    parser.add_argument("--baseline-ref", default=None, help="Baseline commit SHA")
    parser.add_argument("--baseline-name", default=None, help="Baseline display name")
    parser.add_argument("--feature-name", default=None, help="Feature branch name")
    parser.add_argument("--feature-ref", default=None, help="Feature commit SHA")
    parser.add_argument("--behind-baseline", type=int, default=0, help="Commits behind baseline")
    parser.add_argument("--corpus-meta", default=None, help="corpus.meta.json written by the extract step")
    parser.add_argument("--class", dest="klass", default="call", help="Replayed method class")
    parser.add_argument("--rps", default="", help="Configured open-loop request rate")
    parser.add_argument("--duration", default="", help="Configured open-loop duration")
    parser.add_argument("--passes", default="", help="Configured closed-loop passes")
    parser.add_argument("--concurrency", default="", help="Configured closed-loop workers")
    parser.add_argument("--warmup-seconds", default="", help="Warmup duration in seconds")
    parser.add_argument("--run-pairs", type=int, default=None, help="Configured number of run pairs")
    parser.add_argument("--max-tracing-requests", default=None, help="Node tracing request limit")
    parser.add_argument("--benchmark-id", default=None, help="Benchmark ID used for OTLP labels")
    parser.add_argument("--grafana-url", default=None, help="Grafana dashboard URL")
    parser.add_argument("--logs-url", default=None, help="Grafana Explore URL for benchmark logs")
    parser.add_argument("--traces-url", default=None, help="Grafana Explore URL for benchmark traces")
    parser.add_argument("--observability-from-ms", type=int, default=None, help="Observability range start")
    parser.add_argument("--observability-to-ms", type=int, default=None, help="Observability range end")
    parser.add_argument(
        "--derek-command",
        default=os.environ.get("DEREK_BENCH_COMMAND"),
        help="Full derek bench command",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.run_pairs is not None and args.run_pairs < 1:
        print("--run-pairs must be greater than zero", file=sys.stderr)
        return 1
    baseline_runs = [load_run(Path(path)) for path in args.baseline_dir]
    feature_runs = [load_run(Path(path)) for path in args.feature_dir]

    summary = build_summary(args, baseline_runs, feature_runs)

    base_url = f"https://github.com/{args.repo}/commit"
    baseline_label = f"[`{summary['baseline']['name']}`]({base_url}/{summary['baseline']['ref']})"
    feature_label = f"[`{summary['feature']['name']}`]({base_url}/{summary['feature']['ref']})"
    markdown = generate_markdown(
        summary,
        baseline_label,
        feature_label,
        behind_baseline=args.behind_baseline,
        repo=args.repo,
        baseline_ref=summary["baseline"]["ref"],
        baseline_name=summary["baseline"]["name"],
        derek_command=args.derek_command,
    )

    with open(args.output_summary, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"Summary written to {args.output_summary}")
    with open(args.output_markdown, "w") as f:
        f.write(markdown)
    print(f"Markdown written to {args.output_markdown}")
    print(f"Parity: {summary['parity']['line']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
