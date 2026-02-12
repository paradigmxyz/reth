#!/usr/bin/env python3
"""Parse reth-bench CSV output and generate a summary JSON + markdown comparison.

Usage:
    bench-reth-summary.py <combined_csv> <gas_csv> \
        --output-summary <summary.json> \
        --output-markdown <comment.md> \
        --baseline-csv <baseline_combined.csv> \
        [--repo <owner/repo>] \
        [--baseline-ref <sha>] \
        [--branch-name <name>] \
        [--branch-sha <sha>]

Generates a paired statistical comparison between baseline (main) and branch.
Matches blocks by number and computes per-block diffs to cancel out gas
variance. Fails if baseline or branch CSV is missing or empty.
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
                    "persistence_wait_us": int(row["persistence_wait"])
                    if row.get("persistence_wait")
                    else None,
                    "execution_cache_wait_us": int(row.get("execution_cache_wait", 0)),
                    "sparse_trie_wait_us": int(row.get("sparse_trie_wait", 0)),
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


def _paired_data(
    baseline: list[dict], branch: list[dict]
) -> tuple[list[tuple[float, float]], list[float], list[float]]:
    """Match blocks and return paired latencies and per-block diffs.

    Returns:
        pairs: list of (baseline_ms, branch_ms) tuples
        lat_diffs_ms: list of branch − baseline latency diffs in ms
        mgas_diffs: list of branch − baseline Mgas/s diffs
    """
    baseline_by_block = {r["block_number"]: r for r in baseline}
    branch_by_block = {r["block_number"]: r for r in branch}
    common_blocks = sorted(set(baseline_by_block) & set(branch_by_block))

    pairs = []
    lat_diffs_ms = []
    mgas_diffs = []
    for bn in common_blocks:
        b = baseline_by_block[bn]
        f = branch_by_block[bn]
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
    branch_runs: list[list[dict]],
) -> dict:
    """Compute paired statistics between baseline and branch runs.

    Each pair (baseline_runs[i], branch_runs[i]) produces per-block diffs.
    All diffs are pooled for the final CI.
    """
    all_pairs = []
    all_lat_diffs = []
    all_mgas_diffs = []
    for baseline, branch in zip(baseline_runs, branch_runs):
        pairs, lat_diffs, mgas_diffs = _paired_data(baseline, branch)
        all_pairs.extend(pairs)
        all_lat_diffs.extend(lat_diffs)
        all_mgas_diffs.extend(mgas_diffs)

    if not all_lat_diffs:
        return {}

    n = len(all_lat_diffs)
    mean_diff = sum(all_lat_diffs) / n
    std_diff = stddev(all_lat_diffs, mean_diff)
    se = std_diff / math.sqrt(n) if n > 0 else 0.0
    ci = T_CRITICAL * se

    # Bootstrap CI on difference-of-percentiles (resample paired blocks)
    base_lats = sorted([p[0] for p in all_pairs])
    branch_lats = sorted([p[1] for p in all_pairs])
    p50_diff = percentile(branch_lats, 50) - percentile(base_lats, 50)
    p90_diff = percentile(branch_lats, 90) - percentile(base_lats, 90)
    p99_diff = percentile(branch_lats, 99) - percentile(base_lats, 99)

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
    }


def compute_summary(combined: list[dict], gas: list[dict]) -> dict:
    """Compute aggregate metrics from parsed CSV data."""
    blocks = len(combined)
    return {
        "blocks": blocks,
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


def change_str(pct: float, ci_pct: float, lower_is_better: bool) -> str:
    """Format change% with paired CI significance.

    Significant if the CI doesn't cross zero (i.e. |pct| > ci_pct).
    """
    significant = abs(pct) > ci_pct
    if not significant:
        emoji = "⚪"
    elif (pct < 0) == lower_is_better:
        emoji = "✅"
    else:
        emoji = "❌"

    return f"{pct:+.2f}% {emoji} (±{ci_pct:.2f}%)"


def generate_comparison_table(
    run1: dict,
    run2: dict,
    paired: dict,
    repo: str,
    baseline_ref: str,
    branch_name: str,
    branch_sha: str,
) -> str:
    """Generate a markdown comparison table between baseline (main) and branch."""
    n = paired["n"]

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
    baseline_label = f"[`main`]({base_url}/{baseline_ref})"
    branch_label = f"[`{branch_name}`]({base_url}/{branch_sha})"

    lines = [
        f"| Metric | {baseline_label} | {branch_label} | Change |",
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


def generate_markdown(
    summary: dict, comparison_table: str,
    behind_main: int = 0, repo: str = "", baseline_ref: str = "",
) -> str:
    """Generate a markdown comment body."""
    lines = ["## Benchmark Results", "", comparison_table]
    if behind_main > 0:
        s = "s" if behind_main > 1 else ""
        diff_link = f"https://github.com/{repo}/compare/{baseline_ref[:12]}...main"
        lines.append("")
        lines.append(f"> ⚠️ Branch is [**{behind_main} commit{s} behind `main`**]({diff_link}). Consider rebasing for accurate results.")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Parse reth-bench ABBA results")
    parser.add_argument(
        "--baseline-csv", nargs="+", required=True,
        help="Baseline combined_latency.csv files (A1, A2)",
    )
    parser.add_argument(
        "--branch-csv", nargs="+", required=True,
        help="Branch combined_latency.csv files (B1, B2)",
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
    parser.add_argument("--branch-name", default=None, help="Branch name")
    parser.add_argument("--branch-sha", default=None, help="Branch commit SHA")
    parser.add_argument("--behind-main", type=int, default=0, help="Commits behind main")
    args = parser.parse_args()

    if len(args.baseline_csv) != len(args.branch_csv):
        print("Must provide equal number of baseline and branch CSVs", file=sys.stderr)
        sys.exit(1)

    baseline_runs = []
    branch_runs = []
    for path in args.baseline_csv:
        data = parse_combined_csv(path)
        if not data:
            print(f"No results in {path}", file=sys.stderr)
            sys.exit(1)
        baseline_runs.append(data)
    for path in args.branch_csv:
        data = parse_combined_csv(path)
        if not data:
            print(f"No results in {path}", file=sys.stderr)
            sys.exit(1)
        branch_runs.append(data)

    gas = parse_gas_csv(args.gas_csv)

    all_baseline = [r for run in baseline_runs for r in run]
    all_branch = [r for run in branch_runs for r in run]

    summary = compute_summary(all_branch, gas)
    with open(args.output_summary, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"Summary written to {args.output_summary}")

    baseline_stats = compute_stats(all_baseline)
    branch_stats = compute_stats(all_branch)
    paired_stats = compute_paired_stats(baseline_runs, branch_runs)

    if not paired_stats:
        print("No common blocks between baseline and branch runs", file=sys.stderr)
        sys.exit(1)

    comparison_table = generate_comparison_table(
        baseline_stats,
        branch_stats,
        paired_stats,
        repo=args.repo,
        baseline_ref=args.baseline_ref or "main",
        branch_name=args.branch_name or "branch",
        branch_sha=args.branch_sha or "unknown",
    )
    print(f"Generated comparison ({paired_stats['n']} paired blocks, "
          f"mean diff {paired_stats['mean_diff_ms']:+.3f}ms ± {paired_stats['ci_ms']:.3f}ms)")

    markdown = generate_markdown(
        summary, comparison_table,
        behind_main=args.behind_main,
        repo=args.repo,
        baseline_ref=args.baseline_ref or "",
    )

    with open(args.output_markdown, "w") as f:
        f.write(markdown)
    print(f"Markdown written to {args.output_markdown}")


if __name__ == "__main__":
    main()
