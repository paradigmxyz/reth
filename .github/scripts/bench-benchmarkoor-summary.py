#!/usr/bin/env python3
"""Summarize benchmarkoor-replay gas/second results."""

from __future__ import annotations

import argparse
import json
import math
import statistics
from collections import defaultdict
from pathlib import Path


def fmt_rate(value: float) -> str:
    if value >= 1_000_000_000:
        return f"{value / 1_000_000_000:.2f} Ggas/s"
    if value >= 1_000_000:
        return f"{value / 1_000_000:.2f} Mgas/s"
    if value >= 1_000:
        return f"{value / 1_000:.2f} Kgas/s"
    return f"{value:.2f} gas/s"


def fmt_duration(seconds: float) -> str:
    if seconds >= 3600:
        return f"{seconds / 3600:.2f} h"
    if seconds >= 60:
        return f"{seconds / 60:.2f} min"
    return f"{seconds:.2f} s"


def fmt_change(value: float | None) -> str:
    if value is None or not math.isfinite(value):
        return "n/a"
    sign = "+" if value >= 0 else ""
    return f"{sign}{value * 100:.2f}%"


def median(values: list[float]) -> float:
    return statistics.median(values) if values else 0.0


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--results", required=True, type=Path)
    parser.add_argument("--output-json", required=True, type=Path)
    parser.add_argument("--output-markdown", required=True, type=Path)
    parser.add_argument("--baseline-name", required=True)
    parser.add_argument("--baseline-ref", required=True)
    parser.add_argument("--feature-name", required=True)
    parser.add_argument("--feature-ref", required=True)
    parser.add_argument("--repo", required=True)
    args = parser.parse_args()

    rows = [
        json.loads(line)
        for line in args.results.read_text().splitlines()
        if line.strip()
    ]
    if not rows:
        raise SystemExit("no benchmark results found")

    normalized = []
    for row in rows:
        if "gas_per_second" not in row:
            row["gas_per_second"] = (
                row.get("testing_gas_per_sec") or row.get("gas_per_sec") or 0.0
            )
        if "gas" not in row:
            row["gas"] = row.get("testing_gas_used") or 0
        if "elapsed_ms" not in row:
            row["elapsed_ms"] = (row.get("testing_elapsed") or 0.0) * 1000.0
        normalized.append(row)

    by_test: dict[str, dict[str, list[dict]]] = defaultdict(lambda: defaultdict(list))
    for row in normalized:
        by_test[row["test"]][row["run_type"]].append(row)

    tests = []
    ratios = []
    for test, groups in sorted(by_test.items()):
        baseline_rates = [float(row["gas_per_second"]) for row in groups.get("baseline", [])]
        feature_rates = [float(row["gas_per_second"]) for row in groups.get("feature", [])]
        baseline_rate = median(baseline_rates)
        feature_rate = median(feature_rates)
        change = None
        if baseline_rate > 0 and feature_rate > 0:
            change = feature_rate / baseline_rate - 1.0
            ratios.append(feature_rate / baseline_rate)
        tests.append(
            {
                "test": test,
                "gas_bucket": (groups.get("feature") or groups.get("baseline") or [{}])[0].get(
                    "gas_bucket", ""
                ),
                "baseline_gas_per_second": baseline_rate,
                "feature_gas_per_second": feature_rate,
                "change": change,
                "baseline_runs": len(baseline_rates),
                "feature_runs": len(feature_rates),
            }
        )

    aggregate: dict[str, dict[str, float]] = {}
    for run_type in ("baseline", "feature"):
        type_rows = [row for row in normalized if row["run_type"] == run_type]
        total_gas = sum(float(row["gas"]) for row in type_rows)
        total_seconds = sum(float(row["elapsed_ms"]) / 1000.0 for row in type_rows)
        total_wall_seconds = sum(
            float(row.get("wall_elapsed_secs") or row.get("total_elapsed_secs") or 0.0)
            for row in type_rows
        )
        aggregate[run_type] = {
            "total_gas": total_gas,
            "total_seconds": total_seconds,
            "total_wall_seconds": total_wall_seconds,
            "gas_per_second": total_gas / total_seconds if total_seconds > 0 else 0.0,
        }

    geomean_change = None
    if ratios:
        geomean = math.exp(sum(math.log(ratio) for ratio in ratios) / len(ratios))
        geomean_change = geomean - 1.0

    total_change = None
    baseline_total = aggregate["baseline"]["gas_per_second"]
    feature_total = aggregate["feature"]["gas_per_second"]
    if baseline_total > 0 and feature_total > 0:
        total_change = feature_total / baseline_total - 1.0
    baseline_wall_total = aggregate["baseline"]["total_wall_seconds"]
    feature_wall_total = aggregate["feature"]["total_wall_seconds"]
    wall_time_change = None
    if baseline_wall_total > 0 and feature_wall_total > 0:
        wall_time_change = feature_wall_total / baseline_wall_total - 1.0

    summary = {
        "baseline": {"name": args.baseline_name, "ref": args.baseline_ref},
        "feature": {"name": args.feature_name, "ref": args.feature_ref},
        "aggregate": aggregate,
        "changes": {
            "geomean_gas_per_second": geomean_change,
            "total_gas_per_second": total_change,
            "total_wall_time": wall_time_change,
        },
        "tests": tests,
        "raw_results": normalized,
    }
    args.output_json.parent.mkdir(parents=True, exist_ok=True)
    args.output_json.write_text(json.dumps(summary, indent=2) + "\n")

    commit_url = f"https://github.com/{args.repo}/commit"
    diff_url = f"https://github.com/{args.repo}/compare/{args.baseline_ref}...{args.feature_ref}"
    md = []
    md.append("# Benchmarkoor Replay\n")
    md.append(f"**Baseline:** [`{args.baseline_name}`]({commit_url}/{args.baseline_ref})\n")
    md.append(
        f"**Feature:** [`{args.feature_name}`]({commit_url}/{args.feature_ref}) ([diff]({diff_url}))\n"
    )
    md.append(f"**Tests:** {len(tests)}\n\n")
    md.append("| Metric | Baseline | Feature | Change |\n")
    md.append("|--------|----------|---------|--------|\n")
    md.append(
        "| Total measured gas/sec | "
        f"{fmt_rate(baseline_total)} | {fmt_rate(feature_total)} | {fmt_change(total_change)} |\n"
    )
    if baseline_wall_total > 0 or feature_wall_total > 0:
        md.append(
            "| Cumulative wall time | "
            f"{fmt_duration(baseline_wall_total)} | {fmt_duration(feature_wall_total)} | "
            f"{fmt_change(wall_time_change)} |\n"
        )
    md.append(
        "| Per-test geomean gas/sec | "
        f"n/a | n/a | {fmt_change(geomean_change)} |\n\n"
    )
    md.append("| Test | Gas | Baseline | Feature | Change |\n")
    md.append("|------|-----|----------|---------|--------|\n")
    for item in tests:
        md.append(
            f"| `{item['test']}` | {item['gas_bucket'] or 'n/a'} | "
            f"{fmt_rate(item['baseline_gas_per_second'])} | "
            f"{fmt_rate(item['feature_gas_per_second'])} | "
            f"{fmt_change(item['change'])} |\n"
        )

    args.output_markdown.write_text("".join(md))


if __name__ == "__main__":
    main()
