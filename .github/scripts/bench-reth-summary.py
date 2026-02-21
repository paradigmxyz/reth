#!/usr/bin/env python3
"""Parse reth-bench CSV output and generate a summary JSON + markdown comparison.

Usage:
    bench-reth-summary.py <combined_csv> <gas_csv> \
        --output-summary <summary.json> \
        --output-markdown <comment.md> \
        --baseline-csv <baseline_combined.csv> \
        [--repo <owner/repo>] \
        [--baseline-ref <sha>] \
        [--feature-name <name>] \
        [--feature-sha <sha>]

Generates a paired statistical comparison between baseline and feature.
Matches blocks by number and computes per-block diffs to cancel out gas
variance. Fails if baseline or feature CSV is missing or empty.
"""

import argparse
import csv
import json
import math
import random
import sys

GIGAGAS = 1_000_000_000
T_CRITICAL = 1.96  # two-tailed 95% confidence
BOOTSTRAP_ITERATIONS = 10_000


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
                    "gas_limit": int(row["gas_limit"]),
                    "transaction_count": int(row["transaction_count"]),
                    "new_payload_latency_us": int(row["new_payload_latency"]),
                    "fcu_latency_us": int(row["fcu_latency"]),
                    "total_latency_us": int(row["total_latency"]),
                    "persistence_wait_us": _opt_int(row, "persistence_wait"),
                    "execution_cache_wait_us": _opt_int(row, "execution_cache_wait"),
                    "sparse_trie_wait_us": _opt_int(row, "sparse_trie_wait"),
                }
            )
    return rows


def parse_gas_csv(path: str) -> list[dict]:
    """Parse total_gas.csv into a list of per-block dicts."""
    rows = []
    with open(path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append(
                {
                    "block_number": int(row["block_number"]),
                    "gas_used": int(row["gas_used"]),
                    "time_us": int(row["time"]),
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

    return {
        "n": n,
        "mean_ms": mean_lat,
        "stddev_ms": std_lat,
        "p50_ms": percentile(sorted_lat, 50),
        "p90_ms": percentile(sorted_lat, 90),
        "p99_ms": percentile(sorted_lat, 99),
        "mean_mgas_s": mean_mgas_s,
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
) -> tuple[list[tuple[float, float]], list[float], list[float]]:
    """Match blocks and return paired latencies and per-block diffs.

    Returns:
        pairs: list of (baseline_ms, feature_ms) tuples
        lat_diffs_ms: list of feature − baseline latency diffs in ms
        mgas_diffs: list of feature − baseline Mgas/s diffs
    """
    baseline_by_block = {r["block_number"]: r for r in baseline}
    feature_by_block = {r["block_number"]: r for r in feature}
    common_blocks = sorted(set(baseline_by_block) & set(feature_by_block))

    pairs = []
    lat_diffs_ms = []
    mgas_diffs = []
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
    return pairs, lat_diffs_ms, mgas_diffs


def compute_paired_stats(
    baseline_runs: list[list[dict]],
    feature_runs: list[list[dict]],
) -> dict:
    """Compute paired statistics between baseline and feature runs.

    Each pair (baseline_runs[i], feature_runs[i]) produces per-block diffs.
    All diffs are pooled for the final CI.
    """
    all_pairs = []
    all_lat_diffs = []
    all_mgas_diffs = []
    blocks_per_pair = []
    for baseline, feature in zip(baseline_runs, feature_runs):
        pairs, lat_diffs, mgas_diffs = _paired_data(baseline, feature)
        all_pairs.extend(pairs)
        all_lat_diffs.extend(lat_diffs)
        all_mgas_diffs.extend(mgas_diffs)
        blocks_per_pair.append(len(pairs))

    if not all_lat_diffs:
        return {}

    n = len(all_lat_diffs)
    mean_diff = sum(all_lat_diffs) / n
    std_diff = stddev(all_lat_diffs, mean_diff)
    se = std_diff / math.sqrt(n) if n > 0 else 0.0
    ci = T_CRITICAL * se

    # Bootstrap CI on difference-of-percentiles (resample paired blocks)
    base_lats = sorted([p[0] for p in all_pairs])
    feature_lats = sorted([p[1] for p in all_pairs])
    p50_diff = percentile(feature_lats, 50) - percentile(base_lats, 50)
    p90_diff = percentile(feature_lats, 90) - percentile(base_lats, 90)
    p99_diff = percentile(feature_lats, 99) - percentile(base_lats, 99)

    rng = random.Random(42)
    p50_boot, p90_boot, p99_boot = [], [], []
    for _ in range(BOOTSTRAP_ITERATIONS):
        sample = rng.choices(all_pairs, k=n)
        b_sorted = sorted(p[0] for p in sample)
        f_sorted = sorted(p[1] for p in sample)
        p50_boot.append(percentile(f_sorted, 50) - percentile(b_sorted, 50))
        p90_boot.append(percentile(f_sorted, 90) - percentile(b_sorted, 90))
        p99_boot.append(percentile(f_sorted, 99) - percentile(b_sorted, 99))
    p50_boot.sort()
    p90_boot.sort()
    p99_boot.sort()
    lo = int(BOOTSTRAP_ITERATIONS * 0.025)
    hi = int(BOOTSTRAP_ITERATIONS * 0.975)

    mean_mgas_diff = sum(all_mgas_diffs) / len(all_mgas_diffs) if all_mgas_diffs else 0.0
    std_mgas_diff = stddev(all_mgas_diffs, mean_mgas_diff) if len(all_mgas_diffs) > 1 else 0.0
    mgas_se = std_mgas_diff / math.sqrt(len(all_mgas_diffs)) if all_mgas_diffs else 0.0
    mgas_ci = T_CRITICAL * mgas_se

    return {
        "n": n,
        "mean_diff_ms": mean_diff,
        "ci_ms": ci,
        "p50_diff_ms": p50_diff,
        "p50_ci_ms": (p50_boot[hi] - p50_boot[lo]) / 2,
        "p90_diff_ms": p90_diff,
        "p90_ci_ms": (p90_boot[hi] - p90_boot[lo]) / 2,
        "p99_diff_ms": p99_diff,
        "p99_ci_ms": (p99_boot[hi] - p99_boot[lo]) / 2,
        "mean_mgas_diff": mean_mgas_diff,
        "mgas_ci": mgas_ci,
        "blocks": max(blocks_per_pair),
    }



def format_duration(seconds: float) -> str:
    if seconds >= 60:
        return f"{seconds / 60:.1f}min"
    return f"{seconds}s"


def format_gas(gas: int) -> str:
    if gas >= GIGAGAS:
        return f"{gas / GIGAGAS:.1f}G"
    if gas >= 1_000_000:
        return f"{gas / 1_000_000:.1f}M"
    return f"{gas:,}"



def fmt_ms(v: float) -> str:
    return f"{v:.2f}ms"


def fmt_mgas(v: float) -> str:
    return f"{v:.2f}"


def significance(pct: float, ci_pct: float, lower_is_better: bool) -> str:
    """Return significance label: 'good', 'bad', or 'neutral'."""
    significant = abs(pct) > ci_pct
    if not significant:
        return "neutral"
    elif (pct < 0) == lower_is_better:
        return "good"
    else:
        return "bad"


def change_str(pct: float, ci_pct: float, lower_is_better: bool) -> str:
    """Format change% with paired CI significance.

    Significant if the CI doesn't cross zero (i.e. |pct| > ci_pct).
    """
    sig = significance(pct, ci_pct, lower_is_better)
    emoji = {"good": "✅", "bad": "❌", "neutral": "⚪"}[sig]
    return f"{pct:+.2f}% {emoji} (±{ci_pct:.2f}%)"


def compute_changes(
    baseline_stats: dict, feature_stats: dict, paired_stats: dict
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
    ]
    changes = {}
    for name, stat_key, ci_key, base_key, lower_is_better in metrics:
        p = pct(baseline_stats[stat_key], feature_stats[stat_key])
        c = ci_pct(paired_stats[ci_key], baseline_stats[base_key])
        changes[name] = {
            "pct": round(p, 4),
            "ci_pct": round(c, 4),
            "sig": significance(p, c, lower_is_better),
        }
    return changes


def generate_comparison_table(
    run1: dict,
    run2: dict,
    paired: dict,
    repo: str,
    baseline_ref: str,
    baseline_name: str,
    feature_name: str,
    feature_sha: str,
) -> str:
    """Generate a markdown comparison table between baseline and feature."""
    n = paired["blocks"]

    def pct(base: float, feat: float) -> float:
        return (feat - base) / base * 100.0 if base > 0 else 0.0

    mean_pct = pct(run1["mean_ms"], run2["mean_ms"])
    gas_pct = pct(run1["mean_mgas_s"], run2["mean_mgas_s"])

    p50_pct = pct(run1["p50_ms"], run2["p50_ms"])
    p90_pct = pct(run1["p90_ms"], run2["p90_ms"])
    p99_pct = pct(run1["p99_ms"], run2["p99_ms"])

    # Bootstrap CIs as % of baseline percentile
    p50_ci_pct = paired["p50_ci_ms"] / run1["p50_ms"] * 100.0 if run1["p50_ms"] > 0 else 0.0
    p90_ci_pct = paired["p90_ci_ms"] / run1["p90_ms"] * 100.0 if run1["p90_ms"] > 0 else 0.0
    p99_ci_pct = paired["p99_ci_ms"] / run1["p99_ms"] * 100.0 if run1["p99_ms"] > 0 else 0.0

    # CI as a percentage of baseline mean
    lat_ci_pct = paired["ci_ms"] / run1["mean_ms"] * 100.0 if run1["mean_ms"] > 0 else 0.0
    mgas_ci_pct = paired["mgas_ci"] / run1["mean_mgas_s"] * 100.0 if run1["mean_mgas_s"] > 0 else 0.0

    base_url = f"https://github.com/{repo}/commit"
    baseline_label = f"[`{baseline_name}`]({base_url}/{baseline_ref})"
    feature_label = f"[`{feature_name}`]({base_url}/{feature_sha})"

    lines = [
        f"| Metric | {baseline_label} | {feature_label} | Change |",
        "|--------|------|--------|--------|",
        f"| Mean | {fmt_ms(run1['mean_ms'])} | {fmt_ms(run2['mean_ms'])} | {change_str(mean_pct, lat_ci_pct, lower_is_better=True)} |",
        f"| StdDev | {fmt_ms(run1['stddev_ms'])} | {fmt_ms(run2['stddev_ms'])} | |",
        f"| P50 | {fmt_ms(run1['p50_ms'])} | {fmt_ms(run2['p50_ms'])} | {change_str(p50_pct, p50_ci_pct, lower_is_better=True)} |",
        f"| P90 | {fmt_ms(run1['p90_ms'])} | {fmt_ms(run2['p90_ms'])} | {change_str(p90_pct, p90_ci_pct, lower_is_better=True)} |",
        f"| P99 | {fmt_ms(run1['p99_ms'])} | {fmt_ms(run2['p99_ms'])} | {change_str(p99_pct, p99_ci_pct, lower_is_better=True)} |",
        f"| Mgas/s | {fmt_mgas(run1['mean_mgas_s'])} | {fmt_mgas(run2['mean_mgas_s'])} | {change_str(gas_pct, mgas_ci_pct, lower_is_better=False)} |",
        "",
        f"*{n} blocks*",
    ]
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
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Parse reth-bench ABBA results")
    parser.add_argument(
        "--baseline-csv", nargs="+", required=True,
        help="Baseline combined_latency.csv files (A1, A2)",
    )
    parser.add_argument(
        "--feature-csv", "--branch-csv", nargs="+", required=True,
        help="Feature combined_latency.csv files (B1, B2)",
    )
    parser.add_argument("--gas-csv", required=True, help="Path to total_gas.csv")
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
    args = parser.parse_args()

    if len(args.baseline_csv) != len(args.feature_csv):
        print("Must provide equal number of baseline and feature CSVs", file=sys.stderr)
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

    gas = parse_gas_csv(args.gas_csv)

    all_baseline = [r for run in baseline_runs for r in run]
    all_feature = [r for run in feature_runs for r in run]

    baseline_stats = compute_stats(all_baseline)
    feature_stats = compute_stats(all_feature)
    paired_stats = compute_paired_stats(baseline_runs, feature_runs)

    if not paired_stats:
        print("No common blocks between baseline and feature runs", file=sys.stderr)
        sys.exit(1)

    baseline_ref = args.baseline_ref or "main"
    baseline_name = args.baseline_name or "baseline"
    feature_name = args.feature_name or "feature"
    feature_sha = args.feature_ref or "unknown"

    comparison_table = generate_comparison_table(
        baseline_stats,
        feature_stats,
        paired_stats,
        repo=args.repo,
        baseline_ref=baseline_ref,
        baseline_name=baseline_name,
        feature_name=feature_name,
        feature_sha=feature_sha,
    )
    print(f"Generated comparison ({paired_stats['n']} paired blocks, "
          f"mean diff {paired_stats['mean_diff_ms']:+.3f}ms ± {paired_stats['ci_ms']:.3f}ms)")

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
        "blocks": paired_stats["blocks"],
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
        "paired": paired_stats,
        "changes": compute_changes(baseline_stats, feature_stats, paired_stats),
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
    )

    with open(args.output_markdown, "w") as f:
        f.write(markdown)
    print(f"Markdown written to {args.output_markdown}")


if __name__ == "__main__":
    main()
