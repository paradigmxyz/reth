#!/usr/bin/env python3
"""Parse reth-bench CSV output and generate a summary JSON + markdown comparison.

Usage:
    bench-engine-summary.py <combined_csv> <gas_csv> \
        --output-summary <summary.json> \
        --output-markdown <comment.md> \
        [--baseline <baseline.json>]

The baseline file defaults to /reth-bench/baseline.json if it exists.
"""

import argparse
import csv
import json
import os
import sys
from pathlib import Path

GIGAGAS = 1_000_000_000
BASELINE_PATH = Path("/reth-bench/baseline.json")
REGRESSION_THRESHOLD = 0.05  # 5%


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
                "persistence_wait_us": int(row.get("persistence_wait", 0)),
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


def compute_summary(combined: list[dict], gas: list[dict]) -> dict:
    """Compute aggregate metrics from parsed CSV data."""
    total_gas = sum(r["gas_used"] for r in combined)
    blocks = len(combined)

    # Execution-only duration: sum of per-block total_latency
    exec_duration_us = sum(r["total_latency_us"] for r in combined)
    exec_duration_s = exec_duration_us / 1_000_000

    # Wall-clock duration from gas CSV (last timestamp)
    wall_duration_us = gas[-1]["time_us"] if gas else exec_duration_us
    wall_duration_s = wall_duration_us / 1_000_000

    # Per-block Ggas/s
    per_block_ggas = []
    for r in combined:
        lat_s = r["total_latency_us"] / 1_000_000
        if lat_s > 0:
            per_block_ggas.append(r["gas_used"] / lat_s / GIGAGAS)

    np_latencies_ms = sorted(r["new_payload_latency_us"] / 1_000 for r in combined)

    avg_new_payload_ms = sum(np_latencies_ms) / blocks if blocks else 0

    def percentile(sorted_vals: list[float], pct: int) -> float:
        idx = int(len(sorted_vals) * pct / 100)
        idx = min(idx, len(sorted_vals) - 1)
        return sorted_vals[idx] if sorted_vals else 0

    avg_persistence_wait_ms = (
        sum(r["persistence_wait_us"] for r in combined) / blocks / 1_000
        if blocks else 0
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
    """Format duration as human-readable string."""
    if seconds >= 60:
        minutes = seconds / 60
        return f"{minutes:.1f}min"
    return f"{seconds}s"


def format_gas(gas: int) -> str:
    """Format gas as human-readable string (e.g. 60.4G, 123.5M)."""
    if gas >= GIGAGAS:
        return f"{gas / GIGAGAS:.1f}G"
    if gas >= 1_000_000:
        return f"{gas / 1_000_000:.1f}M"
    return f"{gas:,}"


def format_change(current: float, baseline: float) -> str:
    """Format a % change with arrow indicator."""
    if baseline == 0:
        return "N/A"
    pct = (current - baseline) / baseline * 100
    if abs(pct) < 0.5:
        return f"~0%"
    arrow = "🔺" if pct > 0 else "🔻"
    return f"{arrow} {pct:+.1f}%"


def is_regression(current: float, baseline: float) -> bool:
    """Check if a metric regressed beyond threshold (lower is worse for Ggas/s)."""
    if baseline == 0:
        return False
    return (baseline - current) / baseline > REGRESSION_THRESHOLD


def generate_markdown(summary: dict, baseline: dict | None) -> str:
    """Generate a markdown comment body comparing current vs baseline."""
    lines = ["## ⚡ Engine Benchmark Results", ""]

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

    if baseline:
        has_regression = (
            is_regression(summary["mean_ggas_s"], baseline.get("mean_ggas_s", baseline.get("execution_ggas_s", 0)))
            or is_regression(summary["median_block_ggas_s"], baseline.get("median_block_ggas_s", 0))
        )

        if has_regression:
            lines.append("> [!CAUTION]")
            lines.append(f"> Performance regression detected (>{REGRESSION_THRESHOLD*100:.0f}% drop in Ggas/s)")
            lines.append("")

        lines.append("| Metric | This PR | main | Change |")
        lines.append("|--------|---------|------|--------|")

        for label, key, higher_is_better in metrics:
            cur = summary[key]
            base = baseline.get(key, 0)
            if higher_is_better:
                change = format_change(cur, base)
            else:
                change = format_change(base, cur) if base != 0 else "N/A"
            lines.append(f"| {label} | {cur} | {base} | {change} |")

        lines.append("")
        lines.append(f"Blocks: {summary['blocks']} | "
                      f"Total gas: {format_gas(summary['total_gas'])} | "
                      f"Total time: {format_duration(summary['wall_clock_s'])}")
    else:
        lines.append("| Metric | Value |")
        lines.append("|--------|-------|")
        for label, key, _ in metrics:
            lines.append(f"| {label} | {summary[key]} |")
        lines.append("")
        lines.append(f"Blocks: {summary['blocks']} | "
                      f"Total gas: {format_gas(summary['total_gas'])} | "
                      f"Total time: {format_duration(summary['wall_clock_s'])}")
        lines.append("")
        lines.append("*No baseline found — first run on main will establish it.*")

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description="Parse reth-bench results")
    parser.add_argument("combined_csv", help="Path to combined_latency.csv")
    parser.add_argument("gas_csv", help="Path to total_gas.csv")
    parser.add_argument("--output-summary", required=True, help="Output JSON summary path")
    parser.add_argument("--output-markdown", required=True, help="Output markdown path")
    parser.add_argument("--baseline", default=None, help="Baseline JSON path")
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

    # Load baseline
    baseline = None
    baseline_path = Path(args.baseline) if args.baseline else BASELINE_PATH
    if baseline_path.exists():
        with open(baseline_path) as f:
            baseline = json.load(f)
        print(f"Loaded baseline from {baseline_path}")
    else:
        print(f"No baseline found at {baseline_path}")

    markdown = generate_markdown(summary, baseline)

    with open(args.output_markdown, "w") as f:
        f.write(markdown)
    print(f"Markdown written to {args.output_markdown}")


if __name__ == "__main__":
    main()
