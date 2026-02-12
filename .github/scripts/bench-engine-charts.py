#!/usr/bin/env python3
"""Generate benchmark charts from reth-bench CSV output.

Usage:
    bench-engine-charts.py <combined_csv> --output-dir <dir> [--baseline <baseline_csv>]

Generates three PNG charts:
  1. newPayload latency + Ggas/s per block (two subplots)
  2. Wait breakdown (persistence, execution cache, sparse trie) per block
  3. Scatter plot of gas used vs latency

When --baseline is provided, charts overlay both datasets for comparison.
"""

import argparse
import csv
import sys
from pathlib import Path

try:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    import matplotlib.ticker as ticker
except ImportError:
    print("matplotlib is required: pip install matplotlib", file=sys.stderr)
    sys.exit(1)

GIGAGAS = 1_000_000_000


def parse_combined_csv(path: str) -> list[dict]:
    rows = []
    with open(path) as f:
        reader = csv.DictReader(f)
        for row in reader:
            rows.append({
                "block_number": int(row["block_number"]),
                "gas_used": int(row["gas_used"]),
                "new_payload_latency_us": int(row["new_payload_latency"]),
                "persistence_wait_us": int(row["persistence_wait"]) if row.get("persistence_wait") else None,
                "execution_cache_wait_us": int(row.get("execution_cache_wait", 0)),
                "sparse_trie_wait_us": int(row.get("sparse_trie_wait", 0)),
            })
    return rows


def plot_latency_and_throughput(feature: list[dict], baseline: list[dict] | None, out: Path):
    fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(12, 8), sharex=True)

    feat_x = [r["block_number"] for r in feature]
    feat_lat = [r["new_payload_latency_us"] / 1_000 for r in feature]
    feat_ggas = []
    for r in feature:
        lat_s = r["new_payload_latency_us"] / 1_000_000
        feat_ggas.append(r["gas_used"] / lat_s / GIGAGAS if lat_s > 0 else 0)

    if baseline:
        base_x = [r["block_number"] for r in baseline]
        base_lat = [r["new_payload_latency_us"] / 1_000 for r in baseline]
        base_ggas = []
        for r in baseline:
            lat_s = r["new_payload_latency_us"] / 1_000_000
            base_ggas.append(r["gas_used"] / lat_s / GIGAGAS if lat_s > 0 else 0)
        ax1.plot(base_x, base_lat, linewidth=0.8, color="#aaaaaa", label="main", alpha=0.7)
        ax2.plot(base_x, base_ggas, linewidth=0.8, color="#aaaaaa", label="main", alpha=0.7)

    ax1.plot(feat_x, feat_lat, linewidth=0.8, color="#1f77b4", label="feature")
    ax1.set_ylabel("Latency (ms)")
    ax1.set_title("newPayload Latency per Block")
    ax1.grid(True, alpha=0.3)
    if baseline:
        ax1.legend()

    ax2.plot(feat_x, feat_ggas, linewidth=0.8, color="#2ca02c", label="feature")
    ax2.set_xlabel("Block Number")
    ax2.set_ylabel("Ggas/s")
    ax2.set_title("Execution Throughput per Block")
    ax2.grid(True, alpha=0.3)
    if baseline:
        ax2.legend()

    fig.tight_layout()
    fig.savefig(out, dpi=150)
    plt.close(fig)


def plot_wait_breakdown(feature: list[dict], baseline: list[dict] | None, out: Path):
    series = [
        ("Persistence Wait", "persistence_wait_us", "#d62728"),
        ("State Cache Wait", "execution_cache_wait_us", "#ff7f0e"),
        ("Trie Cache Wait", "sparse_trie_wait_us", "#9467bd"),
    ]

    fig, axes = plt.subplots(len(series), 1, figsize=(12, 3 * len(series)), sharex=True)
    for ax, (label, key, color) in zip(axes, series):
        if baseline:
            bx = [r["block_number"] for r in baseline if r[key] is not None]
            by = [r[key] / 1_000 for r in baseline if r[key] is not None]
            if bx:
                ax.plot(bx, by, linewidth=0.8, color="#aaaaaa", label="main", alpha=0.7)

        fx = [r["block_number"] for r in feature if r[key] is not None]
        fy = [r[key] / 1_000 for r in feature if r[key] is not None]
        if fx:
            ax.plot(fx, fy, linewidth=0.8, color=color, label="feature")

        ax.set_ylabel("ms")
        ax.set_title(label)
        ax.grid(True, alpha=0.3)
        if baseline:
            ax.legend()

    axes[-1].set_xlabel("Block Number")
    fig.suptitle("Wait Time Breakdown per Block", fontsize=14, y=1.01)
    fig.tight_layout()
    fig.savefig(out, dpi=150, bbox_inches="tight")
    plt.close(fig)


def plot_gas_vs_latency(feature: list[dict], baseline: list[dict] | None, out: Path):
    fig, ax = plt.subplots(figsize=(8, 6))

    if baseline:
        bgas = [r["gas_used"] / 1_000_000 for r in baseline]
        blat = [r["new_payload_latency_us"] / 1_000 for r in baseline]
        ax.scatter(bgas, blat, s=8, alpha=0.4, color="#aaaaaa", label="main")

    fgas = [r["gas_used"] / 1_000_000 for r in feature]
    flat = [r["new_payload_latency_us"] / 1_000 for r in feature]
    ax.scatter(fgas, flat, s=8, alpha=0.6, color="#1f77b4", label="feature")

    ax.set_xlabel("Gas Used (Mgas)")
    ax.set_ylabel("newPayload Latency (ms)")
    ax.set_title("Gas Used vs Latency")
    ax.grid(True, alpha=0.3)
    if baseline:
        ax.legend()
    fig.tight_layout()
    fig.savefig(out, dpi=150)
    plt.close(fig)


def main():
    parser = argparse.ArgumentParser(description="Generate benchmark charts")
    parser.add_argument("combined_csv", help="Path to combined_latency.csv (feature)")
    parser.add_argument("--output-dir", required=True, help="Output directory for PNG charts")
    parser.add_argument("--baseline", help="Path to baseline (main) combined_latency.csv")
    args = parser.parse_args()

    feature = parse_combined_csv(args.combined_csv)
    if not feature:
        print("No results found in combined CSV", file=sys.stderr)
        sys.exit(1)

    baseline = None
    if args.baseline:
        baseline = parse_combined_csv(args.baseline)
        if not baseline:
            print("Warning: no results in baseline CSV, skipping comparison", file=sys.stderr)
            baseline = None

    out_dir = Path(args.output_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    plot_latency_and_throughput(feature, baseline, out_dir / "latency_throughput.png")
    plot_wait_breakdown(feature, baseline, out_dir / "wait_breakdown.png")
    plot_gas_vs_latency(feature, baseline, out_dir / "gas_vs_latency.png")

    print(f"Charts written to {out_dir}")


if __name__ == "__main__":
    main()
