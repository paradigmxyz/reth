#!/usr/bin/env python3
"""Parse benchmark CSV output and generate a summary JSON + markdown comparison.

Usage:
    bench-reth-summary.py \
        --output-summary <summary.json> \
        --output-markdown <comment.md> \
        --baseline-csv <baseline_combined.csv> [<baseline_combined.csv> ...] \
        [--repo <owner/repo>] \
        [--baseline-ref <sha>] \
        [--feature-name <name>] \
        [--feature-sha <sha>]

Generates a statistical comparison between baseline and feature. Point estimates
use pooled baseline and feature rows. Confidence intervals use whole-run cluster
bootstrapping when multiple runs are available. Fails if baseline or feature CSV
is missing or empty.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import random
import sys

BOOTSTRAP_ITERATIONS = 10_000
PRACTICAL_FLOOR_PCT = {
    "mean": 0.70,
    "p50": 0.70,
    "p90": 1.35,
    "p99": 5.0,
    "mgas_s": 0.45,
    "wall_clock": 0.70,
    "persist_wait": 5.0,
}


def _opt_int(row: dict, key: str) -> int | None:
    """Return int value for a CSV field, or None if missing/empty."""
    v = row.get(key)
    if v is None or v == "":
        return None
    return int(v)


def parse_combined_csv(path: str) -> list[dict]:
    """Parse combined_latency.csv into a list of per-block dicts."""
    rows = []
    with open(path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append(
                {
                    "block_number": int(row["block_number"]),
                    "gas_used": int(row["gas_used"]),
                    "new_payload_latency_us": int(row["new_payload_latency"]),
                    "total_latency_us": int(row["total_latency"]),
                    "persistence_wait_us": _opt_int(row, "persistence_wait"),
                    "execution_cache_wait_us": _opt_int(row, "execution_cache_wait"),
                    "sparse_trie_wait_us": _opt_int(row, "sparse_trie_wait"),
                }
            )
    return rows


def stddev(values: list[float], mean: float) -> float:
    if len(values) < 2:
        return 0.0
    return math.sqrt(sum((v - mean) ** 2 for v in values) / (len(values) - 1))


def percentile(sorted_vals: list[float], pct: int) -> float:
    if not sorted_vals:
        return 0.0
    idx = int(len(sorted_vals) * pct / 100)
    idx = min(idx, len(sorted_vals) - 1)
    return sorted_vals[idx]


def compute_stats(combined: list[dict]) -> dict:
    """Compute per-run statistics from parsed CSV data."""
    n = len(combined)
    if n == 0:
        return {}

    latencies_ms = [r["new_payload_latency_us"] / 1_000 for r in combined]
    sorted_lat = sorted(latencies_ms)
    mean_lat = sum(latencies_ms) / n
    std_lat = stddev(latencies_ms, mean_lat)

    mgas_s_values = []
    for r in combined:
        lat_s = r["new_payload_latency_us"] / 1_000_000
        if lat_s > 0:
            mgas_s_values.append(r["gas_used"] / lat_s / 1_000_000)
    mean_mgas_s = sum(mgas_s_values) / len(mgas_s_values) if mgas_s_values else 0

    total_latencies_ms = [r["total_latency_us"] / 1_000 for r in combined]
    wall_clock_s = sum(total_latencies_ms) / 1_000
    mean_total_lat_ms = sum(total_latencies_ms) / n

    # Persistence wait mean (for main table)
    persist_values_ms = []
    for r in combined:
        v = r.get("persistence_wait_us")
        if v is not None:
            persist_values_ms.append(v / 1_000)
    mean_persist_ms = sum(persist_values_ms) / len(persist_values_ms) if persist_values_ms else 0.0

    return {
        "n": n,
        "mean_ms": mean_lat,
        "stddev_ms": std_lat,
        "p50_ms": percentile(sorted_lat, 50),
        "p90_ms": percentile(sorted_lat, 90),
        "p99_ms": percentile(sorted_lat, 99),
        "mean_mgas_s": mean_mgas_s,
        "wall_clock_s": wall_clock_s,
        "mean_total_lat_ms": mean_total_lat_ms,
        "mean_persist_ms": mean_persist_ms,
    }


def compute_wait_stats(combined: list[dict], field: str) -> dict:
    """Compute mean/p50/p95 for a wait time field (in ms)."""
    values_ms = []
    for r in combined:
        v = r.get(field)
        if v is not None:
            values_ms.append(v / 1_000)
    if not values_ms:
        return {}
    n = len(values_ms)
    mean_val = sum(values_ms) / n
    sorted_vals = sorted(values_ms)
    return {
        "mean_ms": mean_val,
        "p50_ms": percentile(sorted_vals, 50),
        "p95_ms": percentile(sorted_vals, 95),
    }


def _paired_data(
    baseline: list[dict], feature: list[dict]
) -> tuple[list[tuple[float, float]], list[float], list[float], list[float], list[float]]:
    """Match blocks and return paired latencies and per-block diffs.

    Returns:
        pairs: list of (baseline_ms, feature_ms) tuples
        lat_diffs_ms: list of feature − baseline latency diffs in ms
        mgas_diffs: list of feature − baseline Mgas/s diffs
        total_lat_diffs_ms: list of feature − baseline total latency diffs in ms
        persist_diffs_ms: list of feature − baseline persistence wait diffs in ms
    """
    baseline_by_block = {r["block_number"]: r for r in baseline}
    feature_by_block = {r["block_number"]: r for r in feature}
    common_blocks = sorted(set(baseline_by_block) & set(feature_by_block))

    pairs = []
    lat_diffs_ms = []
    mgas_diffs = []
    total_lat_diffs_ms = []
    persist_diffs_ms = []
    for bn in common_blocks:
        b = baseline_by_block[bn]
        f = feature_by_block[bn]
        b_ms = b["new_payload_latency_us"] / 1_000
        f_ms = f["new_payload_latency_us"] / 1_000
        pairs.append((b_ms, f_ms))
        lat_diffs_ms.append(f_ms - b_ms)
        b_lat_s = b["new_payload_latency_us"] / 1_000_000
        f_lat_s = f["new_payload_latency_us"] / 1_000_000
        if b_lat_s > 0 and f_lat_s > 0:
            mgas_diffs.append(
                f["gas_used"] / f_lat_s / 1_000_000
                - b["gas_used"] / b_lat_s / 1_000_000
            )
        total_lat_diffs_ms.append(
            f["total_latency_us"] / 1_000 - b["total_latency_us"] / 1_000
        )
        b_persist = (b.get("persistence_wait_us") or 0) / 1_000
        f_persist = (f.get("persistence_wait_us") or 0) / 1_000
        persist_diffs_ms.append(f_persist - b_persist)
    return pairs, lat_diffs_ms, mgas_diffs, total_lat_diffs_ms, persist_diffs_ms


def _bootstrap_ci(rng: random.Random, diffs: list[float], n_iter: int = BOOTSTRAP_ITERATIONS) -> float:
    """Compute 95% bootstrap CI half-width for the mean of *diffs*."""
    if len(diffs) < 2:
        return 0.0
    n = len(diffs)
    boot_means = sorted(
        sum(rng.choices(diffs, k=n)) / n for _ in range(n_iter)
    )
    lo = int(n_iter * 0.025)
    hi = int(n_iter * 0.975)
    return (boot_means[hi] - boot_means[lo]) / 2


def _bootstrap_percentile_ci(
    rng: random.Random,
    pairs: list[tuple[float, float]],
    pct: int,
    n_iter: int = BOOTSTRAP_ITERATIONS,
) -> float:
    """Compute 95% bootstrap CI half-width for a difference-of-percentiles."""
    if len(pairs) < 2:
        return 0.0
    n = len(pairs)
    boot_diffs = []
    for _ in range(n_iter):
        sample = rng.choices(pairs, k=n)
        b_sorted = sorted(p[0] for p in sample)
        f_sorted = sorted(p[1] for p in sample)
        boot_diffs.append(percentile(f_sorted, pct) - percentile(b_sorted, pct))
    boot_diffs.sort()
    lo = int(n_iter * 0.025)
    hi = int(n_iter * 0.975)
    return (boot_diffs[hi] - boot_diffs[lo]) / 2


def _ci_half_width(samples: list[float]) -> float:
    """Return the 95% CI half-width from sorted bootstrap samples."""
    if len(samples) < 2:
        return 0.0
    samples.sort()
    n = len(samples)
    lo = int(n * 0.025)
    hi = int(n * 0.975)
    return (samples[hi] - samples[lo]) / 2


def _mean(values: list[float]) -> float:
    return sum(values) / len(values) if values else 0.0


def _per_run_metric_values(runs: list[list[dict]]) -> dict[str, list[float]]:
    """Compute one metric value per benchmark run for cluster bootstrap."""
    values = {
        "mean_ms": [],
        "p50_ms": [],
        "p90_ms": [],
        "p99_ms": [],
        "mgas": [],
        "wall_clock_ms": [],
        "persist_ms": [],
    }
    for run in runs:
        stats = compute_stats(run)
        values["mean_ms"].append(stats["mean_ms"])
        values["p50_ms"].append(stats["p50_ms"])
        values["p90_ms"].append(stats["p90_ms"])
        values["p99_ms"].append(stats["p99_ms"])
        values["mgas"].append(stats["mean_mgas_s"])
        values["wall_clock_ms"].append(stats["mean_total_lat_ms"])
        values["persist_ms"].append(stats["mean_persist_ms"])
    return values


def _cluster_bootstrap_ci(
    rng: random.Random,
    baseline_runs: list[list[dict]],
    feature_runs: list[list[dict]],
    n_iter: int = BOOTSTRAP_ITERATIONS,
) -> dict[str, float]:
    """Compute run-cluster bootstrap CIs.

    Each bootstrap sample resamples whole baseline and feature runs with
    replacement. This estimates run-to-run noise without expanding reused
    baseline/feature runs into independent block-level datapoints.
    """
    metrics = ("mean_ms", "p50_ms", "p90_ms", "p99_ms", "mgas", "wall_clock_ms", "persist_ms")
    empty = {metric: 0.0 for metric in metrics}
    if len(baseline_runs) < 2 or len(feature_runs) < 2:
        return empty

    baseline_values = _per_run_metric_values(baseline_runs)
    feature_values = _per_run_metric_values(feature_runs)
    samples = {metric: [] for metric in metrics}
    baseline_count = len(baseline_runs)
    feature_count = len(feature_runs)

    for _ in range(n_iter):
        baseline_indexes = [rng.randrange(baseline_count) for _ in range(baseline_count)]
        feature_indexes = [rng.randrange(feature_count) for _ in range(feature_count)]
        for metric in metrics:
            baseline_sample = [baseline_values[metric][i] for i in baseline_indexes]
            feature_sample = [feature_values[metric][i] for i in feature_indexes]
            samples[metric].append(_mean(feature_sample) - _mean(baseline_sample))

    return {metric: _ci_half_width(samples[metric]) for metric in metrics}


def compute_ci_stats(
    baseline_runs: list[list[dict]],
    feature_runs: list[list[dict]],
) -> dict:
    """Compute confidence interval half-widths for displayed changes.

    Multiple-run comparisons resample whole runs so reused blocks are not
    treated as independent observations. Single-run comparisons fall back to
    block-level bootstrap over matched block numbers.
    """
    if not baseline_runs or not feature_runs:
        return {}

    blocks = max(len(run) for run in baseline_runs + feature_runs)
    rng = random.Random(42)

    if len(baseline_runs) >= 2 and len(feature_runs) >= 2:
        cluster_ci = _cluster_bootstrap_ci(rng, baseline_runs, feature_runs)
        ci = cluster_ci["mean_ms"]
        p50_ci = cluster_ci["p50_ms"]
        p90_ci = cluster_ci["p90_ms"]
        p99_ci = cluster_ci["p99_ms"]
        mgas_ci = cluster_ci["mgas"]
        wall_clock_ci_ms = cluster_ci["wall_clock_ms"]
        persist_ci_ms = cluster_ci["persist_ms"]
    else:
        pairs, all_lat_diffs, all_mgas_diffs, all_total_lat_diffs, all_persist_diffs = (
            _paired_data(baseline_runs[0], feature_runs[0])
        )
        if not all_lat_diffs:
            return {}
        ci = _bootstrap_ci(rng, all_lat_diffs)
        p50_ci = _bootstrap_percentile_ci(rng, pairs, 50)
        p90_ci = _bootstrap_percentile_ci(rng, pairs, 90)
        p99_ci = _bootstrap_percentile_ci(rng, pairs, 99)
        mgas_ci = _bootstrap_ci(rng, all_mgas_diffs) if all_mgas_diffs else 0.0
        wall_clock_ci_ms = _bootstrap_ci(rng, all_total_lat_diffs) if all_total_lat_diffs else 0.0
        persist_ci_ms = _bootstrap_ci(rng, all_persist_diffs) if all_persist_diffs else 0.0

    return {
        "ci_ms": ci,
        "p50_ci_ms": p50_ci,
        "p90_ci_ms": p90_ci,
        "p99_ci_ms": p99_ci,
        "mgas_ci": mgas_ci,
        "wall_clock_ci_ms": wall_clock_ci_ms,
        "persist_ci_ms": persist_ci_ms,
        "blocks": blocks,
    }


def fmt_ms(v: float) -> str:
    return f"{v:.2f}ms"


def fmt_mgas(v: float) -> str:
    return f"{v:.2f}"


def fmt_s(v: float) -> str:
    return f"{v:.2f}s"


def display_bal_mode(bal_mode: str | None) -> str | None:
    if not bal_mode or bal_mode == "false":
        return None
    if bal_mode == "both":
        return "true"
    return bal_mode


def practical_floor_pct(metric: str, _baseline_value: float) -> float:
    """Return the practical significance floor as a percent of baseline."""
    return PRACTICAL_FLOOR_PCT.get(metric, 0.0)


def significance(pct: float, ci_pct: float, floor_pct: float, lower_is_better: bool) -> str:
    """Return significance label: 'good', 'bad', or 'neutral'.

    A result is only significant if the whole confidence interval clears a
    practical significance floor. The floor is the same for every run shape;
    higher run counts only tighten the CI.
    """
    improvement_pct = -pct if lower_is_better else pct
    if improvement_pct - ci_pct > floor_pct:
        return "good"
    if improvement_pct + ci_pct < -floor_pct:
        return "bad"
    return "neutral"


def change_str(pct: float, ci_pct: float, floor_pct: float, lower_is_better: bool) -> str:
    """Format change% with CI significance.

    Significant if the confidence interval clears the practical floor.
    """
    sig = significance(pct, ci_pct, floor_pct, lower_is_better)
    emoji = {"good": "✅", "bad": "❌", "neutral": "⚪"}[sig]
    return f"{pct:+.2f}% {emoji} (±{ci_pct:.2f}%, floor {floor_pct:.2f}%)"


def compute_changes(
    baseline_stats: dict, feature_stats: dict, ci_stats: dict
) -> dict:
    """Pre-compute change percentages and significance for each metric."""
    def pct(base: float, feat: float) -> float:
        return (feat - base) / base * 100.0 if base > 0 else 0.0

    def ci_pct(ci_ms: float, base_ms: float) -> float:
        return ci_ms / base_ms * 100.0 if base_ms > 0 else 0.0

    metrics = [
        ("mean", "mean_ms", "ci_ms", "mean_ms", True),
        ("p50", "p50_ms", "p50_ci_ms", "p50_ms", True),
        ("p90", "p90_ms", "p90_ci_ms", "p90_ms", True),
        ("p99", "p99_ms", "p99_ci_ms", "p99_ms", True),
        ("mgas_s", "mean_mgas_s", "mgas_ci", "mean_mgas_s", False),
        ("wall_clock", "wall_clock_s", "wall_clock_ci_ms", "mean_total_lat_ms", True),
        ("persist_wait", "mean_persist_ms", "persist_ci_ms", "mean_persist_ms", True),
    ]
    changes = {}
    for name, stat_key, ci_key, base_key, lower_is_better in metrics:
        p = pct(baseline_stats[stat_key], feature_stats[stat_key])
        c = ci_pct(ci_stats[ci_key], baseline_stats[base_key])
        floor = practical_floor_pct(name, baseline_stats[base_key])
        changes[name] = {
            "pct": round(p, 4),
            "ci_pct": round(c, 4),
            "floor_pct": round(floor, 4),
            "sig": significance(p, c, floor, lower_is_better),
        }
    return changes


def generate_comparison_table(
    run1: dict,
    run2: dict,
    ci_stats: dict,
    repo: str,
    baseline_ref: str,
    baseline_name: str,
    feature_name: str,
    feature_sha: str,
    big_blocks: bool = False,
    warmup_blocks: str | None = None,
    wait_time: str | None = None,
    bal_mode: str | None = None,
    run_pairs: int | None = None,
) -> str:
    """Generate a markdown comparison table between baseline and feature."""
    n = ci_stats["blocks"]

    def pct(base: float, feat: float) -> float:
        return (feat - base) / base * 100.0 if base > 0 else 0.0

    gas_pct = pct(run1["mean_mgas_s"], run2["mean_mgas_s"])
    wall_pct = pct(run1["wall_clock_s"], run2["wall_clock_s"])

    mean_pct = pct(run1["mean_ms"], run2["mean_ms"])
    p50_pct = pct(run1["p50_ms"], run2["p50_ms"])
    p90_pct = pct(run1["p90_ms"], run2["p90_ms"])
    p99_pct = pct(run1["p99_ms"], run2["p99_ms"])

    persist_pct = pct(run1["mean_persist_ms"], run2["mean_persist_ms"])

    # Bootstrap CIs as % of baseline percentile
    mean_ci_pct = ci_stats["ci_ms"] / run1["mean_ms"] * 100.0 if run1["mean_ms"] > 0 else 0.0
    p50_ci_pct = ci_stats["p50_ci_ms"] / run1["p50_ms"] * 100.0 if run1["p50_ms"] > 0 else 0.0
    p90_ci_pct = ci_stats["p90_ci_ms"] / run1["p90_ms"] * 100.0 if run1["p90_ms"] > 0 else 0.0
    p99_ci_pct = ci_stats["p99_ci_ms"] / run1["p99_ms"] * 100.0 if run1["p99_ms"] > 0 else 0.0

    # CI as a percentage of baseline
    mgas_ci_pct = ci_stats["mgas_ci"] / run1["mean_mgas_s"] * 100.0 if run1["mean_mgas_s"] > 0 else 0.0
    wall_ci_pct = ci_stats["wall_clock_ci_ms"] / run1["mean_total_lat_ms"] * 100.0 if run1["mean_total_lat_ms"] > 0 else 0.0
    persist_ci_pct = ci_stats["persist_ci_ms"] / run1["mean_persist_ms"] * 100.0 if run1["mean_persist_ms"] > 0 else 0.0

    mean_floor = practical_floor_pct("mean", run1["mean_ms"])
    p50_floor = practical_floor_pct("p50", run1["p50_ms"])
    p90_floor = practical_floor_pct("p90", run1["p90_ms"])
    p99_floor = practical_floor_pct("p99", run1["p99_ms"])
    mgas_floor = practical_floor_pct("mgas_s", run1["mean_mgas_s"])
    wall_floor = practical_floor_pct("wall_clock", run1["mean_total_lat_ms"])
    persist_floor = practical_floor_pct("persist_wait", run1["mean_persist_ms"])

    base_url = f"https://github.com/{repo}/commit"
    baseline_label = f"[`{baseline_name}`]({base_url}/{baseline_ref})"
    feature_label = f"[`{feature_name}`]({base_url}/{feature_sha})"

    lines = [
        f"| Metric | {baseline_label} | {feature_label} | Change |",
        "|--------|------|--------|--------|",
        f"| Mean | {fmt_ms(run1['mean_ms'])} | {fmt_ms(run2['mean_ms'])} | {change_str(mean_pct, mean_ci_pct, mean_floor, lower_is_better=True)} |",
        f"| P50 | {fmt_ms(run1['p50_ms'])} | {fmt_ms(run2['p50_ms'])} | {change_str(p50_pct, p50_ci_pct, p50_floor, lower_is_better=True)} |",
        f"| P90 | {fmt_ms(run1['p90_ms'])} | {fmt_ms(run2['p90_ms'])} | {change_str(p90_pct, p90_ci_pct, p90_floor, lower_is_better=True)} |",
        f"| P99 | {fmt_ms(run1['p99_ms'])} | {fmt_ms(run2['p99_ms'])} | {change_str(p99_pct, p99_ci_pct, p99_floor, lower_is_better=True)} |",
        f"| Mgas/s | {fmt_mgas(run1['mean_mgas_s'])} | {fmt_mgas(run2['mean_mgas_s'])} | {change_str(gas_pct, mgas_ci_pct, mgas_floor, lower_is_better=False)} |",
        f"| Wall Clock | {fmt_s(run1['wall_clock_s'])} | {fmt_s(run2['wall_clock_s'])} | {change_str(wall_pct, wall_ci_pct, wall_floor, lower_is_better=True)} |",
        f"| Persist Wait | {fmt_ms(run1['mean_persist_ms'])} | {fmt_ms(run2['mean_persist_ms'])} | {change_str(persist_pct, persist_ci_pct, persist_floor, lower_is_better=True)} |",
        "",
    ]
    meta_parts = [f"{n} {'big blocks' if big_blocks else 'blocks'}"]
    if warmup_blocks:
        meta_parts.append(f"{warmup_blocks} warmup")
    if run_pairs:
        meta_parts.append(f"{run_pairs} run pairs")
    if wait_time:
        meta_parts.append(f"wait time: {wait_time}")
    display_mode = display_bal_mode(bal_mode)
    if big_blocks and display_mode:
        meta_parts.append(f"BAL: {display_mode}")
    lines.append(f"*{', '.join(meta_parts)}*")
    return "\n".join(lines)


def generate_wait_time_table(
    title: str,
    baseline_stats: dict,
    feature_stats: dict,
    baseline_label: str,
    feature_label: str,
) -> str:
    """Generate a markdown table for a wait time metric."""
    if not baseline_stats or not feature_stats:
        return ""
    lines = [
        f"### {title}",
        "",
        f"| Metric | {baseline_label} | {feature_label} |",
        "|--------|------|--------|",
        f"| Mean | {fmt_ms(baseline_stats['mean_ms'])} | {fmt_ms(feature_stats['mean_ms'])} |",
        f"| P50 | {fmt_ms(baseline_stats['p50_ms'])} | {fmt_ms(feature_stats['p50_ms'])} |",
        f"| P95 | {fmt_ms(baseline_stats['p95_ms'])} | {fmt_ms(feature_stats['p95_ms'])} |",
    ]
    return "\n".join(lines)


def generate_markdown(
    summary: dict, comparison_table: str,
    wait_time_tables: list[str] | None = None,
    behind_baseline: int = 0, repo: str = "", baseline_ref: str = "", baseline_name: str = "",
    grafana_url: str | None = None,
) -> str:
    """Generate a markdown comment body."""
    lines = ["## Benchmark Results", ""]
    if behind_baseline > 0:
        s = "s" if behind_baseline > 1 else ""
        diff_link = f"https://github.com/{repo}/compare/{baseline_ref[:12]}...{baseline_name}"
        lines.append(f"> ⚠️ Feature is [**{behind_baseline} commit{s} behind `{baseline_name}`**]({diff_link}). Consider rebasing for accurate results.")
        lines.append("")
    lines.append(comparison_table)
    if wait_time_tables:
        lines.append("")
        lines.append("<details>")
        lines.append("<summary>Wait Time Breakdown</summary>")
        lines.append("")
        for table in wait_time_tables:
            if table:
                lines.append(table)
                lines.append("")
        lines.append("</details>")
    if grafana_url:
        lines.append("")
        lines.append(f"**[Grafana Dashboard]({grafana_url})**")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Parse benchmark run-pair results")
    parser.add_argument(
        "--baseline-csv", nargs="+", required=True,
        help="Baseline combined_latency.csv files",
    )
    parser.add_argument(
        "--feature-csv", "--branch-csv", nargs="+", required=True,
        help="Feature combined_latency.csv files",
    )
    parser.add_argument("--gas-csv", default=None, help=argparse.SUPPRESS)
    parser.add_argument(
        "--output-summary", required=True, help="Output JSON summary path"
    )
    parser.add_argument("--output-markdown", required=True, help="Output markdown path")
    parser.add_argument(
        "--repo", default="paradigmxyz/reth", help="GitHub repo (owner/name)"
    )
    parser.add_argument("--baseline-ref", default=None, help="Baseline commit SHA")
    parser.add_argument("--baseline-name", default=None, help="Baseline display name")
    parser.add_argument("--feature-name", "--branch-name", default=None, help="Feature branch name")
    parser.add_argument("--feature-ref", "--branch-sha", "--feature-sha", default=None, help="Feature commit SHA")
    parser.add_argument("--behind-baseline", "--behind-main", type=int, default=0, help="Commits behind baseline")
    parser.add_argument("--big-blocks", action="store_true", default=False, help="Big blocks mode")
    parser.add_argument("--warmup-blocks", default=None, help="Number of warmup blocks")
    parser.add_argument("--wait-time", default=None, help="Wait time interval used between blocks")
    parser.add_argument("--bal-mode", default=None, help="BAL mode (true, feature, baseline)")
    parser.add_argument("--grafana-url", default=None, help="Grafana dashboard URL for this benchmark run")
    parser.add_argument("--run-pairs", type=int, default=None, help="Configured number of benchmark run pairs")
    args = parser.parse_args()

    if args.run_pairs is not None and args.run_pairs < 1:
        print("--run-pairs must be greater than zero", file=sys.stderr)
        sys.exit(1)

    baseline_runs = []
    feature_runs = []
    for path in args.baseline_csv:
        data = parse_combined_csv(path)
        if not data:
            print(f"No results in {path}", file=sys.stderr)
            sys.exit(1)
        baseline_runs.append(data)
    for path in args.feature_csv:
        data = parse_combined_csv(path)
        if not data:
            print(f"No results in {path}", file=sys.stderr)
            sys.exit(1)
        feature_runs.append(data)

    all_baseline = [r for run in baseline_runs for r in run]
    all_feature = [r for run in feature_runs for r in run]

    baseline_stats = compute_stats(all_baseline)
    feature_stats = compute_stats(all_feature)
    ci_stats = compute_ci_stats(baseline_runs, feature_runs)

    if not ci_stats:
        print("No comparable baseline and feature results", file=sys.stderr)
        sys.exit(1)

    baseline_ref = args.baseline_ref or "main"
    baseline_name = args.baseline_name or "baseline"
    feature_name = args.feature_name or "feature"
    feature_sha = args.feature_ref or "unknown"
    bal_mode = display_bal_mode(args.bal_mode)

    comparison_table = generate_comparison_table(
        baseline_stats,
        feature_stats,
        ci_stats,
        repo=args.repo,
        baseline_ref=baseline_ref,
        baseline_name=baseline_name,
        feature_name=feature_name,
        feature_sha=feature_sha,
        big_blocks=args.big_blocks,
        warmup_blocks=args.warmup_blocks,
        wait_time=args.wait_time,
        bal_mode=bal_mode,
        run_pairs=args.run_pairs,
    )
    print(
        f"Generated comparison ({ci_stats['blocks']} blocks, "
        f"mean CI ± {ci_stats['ci_ms']:.3f}ms)"
    )

    base_url = f"https://github.com/{args.repo}/commit"
    baseline_label = f"[`{baseline_name}`]({base_url}/{baseline_ref})"
    feature_label = f"[`{feature_name}`]({base_url}/{feature_sha})"

    wait_fields = [
        ("persistence_wait_us", "Persistence Wait"),
        ("sparse_trie_wait_us", "Trie Cache Update Wait"),
        ("execution_cache_wait_us", "Execution Cache Update Wait"),
    ]
    wait_time_tables = []
    wait_time_data = {}
    for field, title in wait_fields:
        b_stats = compute_wait_stats(all_baseline, field)
        f_stats = compute_wait_stats(all_feature, field)
        if b_stats and f_stats:
            wait_time_data[field] = {
                "title": title,
                "baseline": b_stats,
                "feature": f_stats,
            }
        table = generate_wait_time_table(title, b_stats, f_stats, baseline_label, feature_label)
        if table:
            wait_time_tables.append(table)

    summary = {
        "blocks": ci_stats["blocks"],
        "big_blocks": args.big_blocks,
        "warmup_blocks": args.warmup_blocks,
        "run_pairs": args.run_pairs,
        "wait_time": args.wait_time,
        "bal_mode": bal_mode,
        "baseline": {
            "name": baseline_name,
            "ref": baseline_ref,
            "stats": baseline_stats,
        },
        "feature": {
            "name": feature_name,
            "ref": feature_sha,
            "stats": feature_stats,
        },
        "changes": compute_changes(baseline_stats, feature_stats, ci_stats),
        "wait_times": wait_time_data,
    }
    with open(args.output_summary, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"Summary written to {args.output_summary}")

    markdown = generate_markdown(
        summary, comparison_table,
        wait_time_tables=wait_time_tables,
        behind_baseline=args.behind_baseline,
        repo=args.repo,
        baseline_ref=baseline_ref,
        baseline_name=baseline_name,
        grafana_url=args.grafana_url,
    )

    with open(args.output_markdown, "w") as f:
        f.write(markdown)
    print(f"Markdown written to {args.output_markdown}")


if __name__ == "__main__":
    main()
