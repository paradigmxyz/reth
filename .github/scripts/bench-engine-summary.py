#!/usr/bin/env python3
"""Parse reth-bench CSV output and generate a summary JSON + markdown comparison.

Usage:
    bench-engine-summary.py <combined_csv> <gas_csv> \
        --output-summary <summary.json> \
        --output-markdown <comment.md> \
        [--baseline-csv <run1_combined.csv>]
        [--baseline <baseline.json>]

When --baseline-csv is provided, generates a statistical comparison table
between run 1 (baseline-csv) and run 2 (combined_csv) with z-score thresholds.
"""

import argparse
import csv
import json
import math
import sys
from pathlib import Path

GIGAGAS = 1_000_000_000
BASELINE_PATH = Path("/reth-bench/baseline.json")
NOISE_THRESHOLD = 1.0  # minimum threshold floor (%)
Z_K = 2.0  # z-score multiplier for 95% confidence interval


def parse_combined_csv(path: str) -> list[dict]:
    """Parse combined_latency.csv into a list of per-block dicts."""
    rows = []
    with open(path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append({
                "block_number": int(row["block_number"]),
                "gas_used": int(row["gas_used"]),
                "gas_limit": int(row["gas_limit"]),
                "transaction_count": int(row["transaction_count"]),
                "new_payload_latency_us": int(row["new_payload_latency"]),
                "fcu_latency_us": int(row["fcu_latency"]),
                "total_latency_us": int(row["total_latency"]),
                "persistence_wait_us": int(row["persistence_wait"]) if row.get("persistence_wait") else None,
                "execution_cache_wait_us": int(row.get("execution_cache_wait", 0)),
                "sparse_trie_wait_us": int(row.get("sparse_trie_wait", 0)),
            })
    return rows


def parse_gas_csv(path: str) -> list[dict]:
    """Parse total_gas.csv into a list of per-block dicts."""
    rows = []
    with open(path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append({
                "block_number": int(row["block_number"]),
                "gas_used": int(row["gas_used"]),
                "time_us": int(row["time"]),
            })
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


def compute_summary(combined: list[dict], gas: list[dict]) -> dict:
    """Compute aggregate metrics from parsed CSV data."""
    total_gas = sum(r["gas_used"] for r in combined)
    blocks = len(combined)

    exec_duration_us = sum(r["total_latency_us"] for r in combined)
    exec_duration_s = exec_duration_us / 1_000_000

    wall_duration_us = gas[-1]["time_us"] if gas else exec_duration_us
    wall_duration_s = wall_duration_us / 1_000_000

    per_block_ggas = []
    for r in combined:
        lat_s = r["total_latency_us"] / 1_000_000
        if lat_s > 0:
            per_block_ggas.append(r["gas_used"] / lat_s / GIGAGAS)

    np_latencies_ms = sorted(r["new_payload_latency_us"] / 1_000 for r in combined)

    avg_new_payload_ms = sum(np_latencies_ms) / blocks if blocks else 0

    persistence_values = [r["persistence_wait_us"] for r in combined if r["persistence_wait_us"] is not None]
    avg_persistence_wait_ms = (
        sum(persistence_values) / len(persistence_values) / 1_000
        if persistence_values else 0
    )
    avg_execution_cache_wait_ms = (
        sum(r["execution_cache_wait_us"] for r in combined) / blocks / 1_000
        if blocks else 0
    )
    avg_sparse_trie_wait_ms = (
        sum(r["sparse_trie_wait_us"] for r in combined) / blocks / 1_000
        if blocks else 0
    )

    sorted_ggas = sorted(per_block_ggas)

    return {
        "blocks": blocks,
        "total_gas": total_gas,
        "wall_clock_s": round(wall_duration_s, 3),
        "execution_s": round(exec_duration_s, 3),
        "mean_ggas_s": round(total_gas / exec_duration_s / GIGAGAS, 4) if exec_duration_s > 0 else 0,
        "median_block_ggas_s": round(percentile(sorted_ggas, 50), 4),
        "avg_new_payload_ms": round(avg_new_payload_ms, 2),
        "p90_new_payload_ms": round(percentile(np_latencies_ms, 90), 2),
        "p95_new_payload_ms": round(percentile(np_latencies_ms, 95), 2),
        "avg_persistence_wait_ms": round(avg_persistence_wait_ms, 2),
        "avg_execution_cache_wait_ms": round(avg_execution_cache_wait_ms, 2),
        "avg_sparse_trie_wait_ms": round(avg_sparse_trie_wait_ms, 2),
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


def z_threshold(std_b: float, std_f: float, n: int, ref_mean: float) -> float:
    """Calculate z-score threshold percentage.
    
    threshold% = k * sqrt(stddev_b² + stddev_f²) / (mean * sqrt(n)) * 100
    """
    if n <= 0 or ref_mean <= 0:
        return 0.0
    combined_std = math.sqrt(std_b ** 2 + std_f ** 2)
    se = combined_std / math.sqrt(n)
    return max(Z_K * se / ref_mean * 100.0, NOISE_THRESHOLD)


def z_threshold_p50(std_b: float, std_f: float, n: int, ref_p50: float) -> float:
    """Z-score threshold for medians (1.253x correction for median SE)."""
    if n <= 0 or ref_p50 <= 0:
        return 0.0
    combined_std = math.sqrt(std_b ** 2 + std_f ** 2)
    se = 1.253 * combined_std / math.sqrt(n)
    return max(Z_K * se / ref_p50 * 100.0, NOISE_THRESHOLD)


def fmt_ms(v: float) -> str:
    return f"{v:.2f}ms"


def fmt_mgas(v: float) -> str:
    return f"{v:.2f}"


def change_str(pct: float, threshold: float, lower_is_better: bool) -> str:
    """Format change% with significance indicator and threshold."""
    if abs(pct) <= threshold:
        emoji = "≈"
    elif (pct < 0) == lower_is_better:
        emoji = "✅"
    else:
        emoji = "❌"

    thresh_str = f" ±{threshold:.2f}%" if threshold > NOISE_THRESHOLD else ""
    return f"{pct:+.2f}% {emoji}{thresh_str}"


def generate_comparison_table(run1: dict, run2: dict) -> str:
    """Generate a markdown comparison table between two runs using z-score thresholds."""
    n = min(run1["n"], run2["n"])

    thresh_mean = z_threshold(run1["stddev_ms"], run2["stddev_ms"], n, run1["mean_ms"])
    thresh_p50 = z_threshold_p50(run1["stddev_ms"], run2["stddev_ms"], n, run1["p50_ms"])
    thresh_gas = z_threshold(run1["stddev_ms"], run2["stddev_ms"], n, run1["mean_ms"])

    def pct(base: float, feat: float) -> float:
        return (feat - base) / base * 100.0 if base > 0 else 0.0

    mean_pct = pct(run1["mean_ms"], run2["mean_ms"])
    p50_pct = pct(run1["p50_ms"], run2["p50_ms"])
    p90_pct = pct(run1["p90_ms"], run2["p90_ms"])
    p99_pct = pct(run1["p99_ms"], run2["p99_ms"])
    gas_pct = pct(run1["mean_mgas_s"], run2["mean_mgas_s"])

    lines = [
        "### Statistical Comparison (run 1 vs run 2)",
        "",
        "| Metric | Run 1 | Run 2 | Change |",
        "|--------|-------|-------|--------|",
        f"| Mean | {fmt_ms(run1['mean_ms'])} | {fmt_ms(run2['mean_ms'])} | {change_str(mean_pct, thresh_mean, lower_is_better=True)} |",
        f"| StdDev | {fmt_ms(run1['stddev_ms'])} | {fmt_ms(run2['stddev_ms'])} | |",
        f"| P50 | {fmt_ms(run1['p50_ms'])} | {fmt_ms(run2['p50_ms'])} | {change_str(p50_pct, thresh_p50, lower_is_better=True)} |",
        f"| P90 | {fmt_ms(run1['p90_ms'])} | {fmt_ms(run2['p90_ms'])} | {change_str(p90_pct, thresh_mean, lower_is_better=True)} |",
        f"| P99 | {fmt_ms(run1['p99_ms'])} | {fmt_ms(run2['p99_ms'])} | {change_str(p99_pct, thresh_mean, lower_is_better=True)} |",
        f"| Mgas/s | {fmt_mgas(run1['mean_mgas_s'])} | {fmt_mgas(run2['mean_mgas_s'])} | {change_str(gas_pct, thresh_gas, lower_is_better=False)} |",
        "",
        f"*{n} blocks, z-score k={Z_K} (95% CI), noise floor {NOISE_THRESHOLD}%*",
    ]
    return "\n".join(lines)


def generate_markdown(summary: dict, baseline: dict | None, comparison_table: str | None) -> str:
    """Generate a markdown comment body."""
    lines = ["## ⚡ Engine Benchmark Results", ""]

    if comparison_table:
        lines.append(comparison_table)
        lines.append("")

    metrics = [
        ("Mean Ggas/s", "mean_ggas_s", True),
        ("Median block Ggas/s", "median_block_ggas_s", True),
        ("Avg newPayload (ms)", "avg_new_payload_ms", False),
        ("P90 newPayload (ms)", "p90_new_payload_ms", False),
        ("P95 newPayload (ms)", "p95_new_payload_ms", False),
        ("Avg persistence wait (ms)", "avg_persistence_wait_ms", False),
        ("Avg state cache wait (ms)", "avg_execution_cache_wait_ms", False),
        ("Avg trie cache wait (ms)", "avg_sparse_trie_wait_ms", False),
    ]

    lines.append("### Run 2 Summary")
    lines.append("")
    lines.append("| Metric | Value |")
    lines.append("|--------|-------|")
    for label, key, _ in metrics:
        lines.append(f"| {label} | {summary[key]} |")
    lines.append("")
    lines.append(f"Blocks: {summary['blocks']} | "
                  f"Total gas: {format_gas(summary['total_gas'])} | "
                  f"Total time: {format_duration(summary['wall_clock_s'])}")

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Parse reth-bench results")
    parser.add_argument("combined_csv", help="Path to combined_latency.csv (run 2)")
    parser.add_argument("gas_csv", help="Path to total_gas.csv (run 2)")
    parser.add_argument("--output-summary", required=True, help="Output JSON summary path")
    parser.add_argument("--output-markdown", required=True, help="Output markdown path")
    parser.add_argument("--baseline", default=None, help="Baseline JSON path (for main comparison)")
    parser.add_argument("--baseline-csv", default=None, help="Run 1 combined_latency.csv for comparison")
    args = parser.parse_args()

    combined = parse_combined_csv(args.combined_csv)
    gas = parse_gas_csv(args.gas_csv)

    if not combined:
        print("No results found in combined CSV", file=sys.stderr)
        sys.exit(1)

    summary = compute_summary(combined, gas)

    with open(args.output_summary, "w") as f:
        json.dump(summary, f, indent=2)
    print(f"Summary written to {args.output_summary}")

    comparison_table = None
    if args.baseline_csv and Path(args.baseline_csv).exists():
        run1_data = parse_combined_csv(args.baseline_csv)
        if run1_data:
            run1_stats = compute_stats(run1_data)
            run2_stats = compute_stats(combined)
            comparison_table = generate_comparison_table(run1_stats, run2_stats)
            print("Generated statistical comparison table")

    baseline = None
    baseline_path = Path(args.baseline) if args.baseline else BASELINE_PATH
    if baseline_path.exists():
        with open(baseline_path) as f:
            baseline = json.load(f)
        print(f"Loaded baseline from {baseline_path}")

    markdown = generate_markdown(summary, baseline, comparison_table)

    with open(args.output_markdown, "w") as f:
        f.write(markdown)
    print(f"Markdown written to {args.output_markdown}")


if __name__ == "__main__":
    main()
