#!/usr/bin/env python3
"""Generate benchmark charts from reth-bench CSV output.

Usage:
    bench-engine-charts.py <combined_csv> --output-dir <dir>

Generates four PNG charts:
  1. newPayload latency per block
  2. Gas/s per block
  3. Wait breakdown (persistence, execution cache, sparse trie) per block
  4. Scatter plot of gas used vs latency
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
                "persistence_wait_us": int(row.get("persistence_wait", 0)),
                "execution_cache_wait_us": int(row.get("execution_cache_wait", 0)),
                "sparse_trie_wait_us": int(row.get("sparse_trie_wait", 0)),
            })
    return rows


def plot_latency(rows: list[dict], out: Path):
    blocks = [r["block_number"] for r in rows]
    latency_ms = [r["new_payload_latency_us"] / 1_000 for r in rows]

    fig, ax = plt.subplots(figsize=(12, 5))
    ax.plot(blocks, latency_ms, linewidth=0.8, color="#1f77b4")
    ax.set_xlabel("Block Number")
    ax.set_ylabel("newPayload Latency (ms)")
    ax.set_title("newPayload Latency per Block")
    ax.grid(True, alpha=0.3)
    fig.tight_layout()
    fig.savefig(out, dpi=150)
    plt.close(fig)


def plot_gas_per_second(rows: list[dict], out: Path):
    blocks = [r["block_number"] for r in rows]
    ggas_s = []
    for r in rows:
        lat_s = r["new_payload_latency_us"] / 1_000_000
        ggas_s.append(r["gas_used"] / lat_s / GIGAGAS if lat_s > 0 else 0)

    fig, ax = plt.subplots(figsize=(12, 5))
    ax.plot(blocks, ggas_s, linewidth=0.8, color="#2ca02c")
    ax.set_xlabel("Block Number")
    ax.set_ylabel("Ggas/s")
    ax.set_title("Execution Throughput per Block")
    ax.grid(True, alpha=0.3)
    fig.tight_layout()
    fig.savefig(out, dpi=150)
    plt.close(fig)


def plot_wait_breakdown(rows: list[dict], out: Path):
    blocks = [r["block_number"] for r in rows]
    persistence_ms = [r["persistence_wait_us"] / 1_000 for r in rows]
    exec_cache_ms = [r["execution_cache_wait_us"] / 1_000 for r in rows]
    sparse_trie_ms = [r["sparse_trie_wait_us"] / 1_000 for r in rows]

    fig, ax = plt.subplots(figsize=(12, 5))
    ax.plot(blocks, persistence_ms, linewidth=0.8, label="Persistence Wait", color="#d62728")
    ax.plot(blocks, exec_cache_ms, linewidth=0.8, label="Execution Cache Wait", color="#ff7f0e")
    ax.plot(blocks, sparse_trie_ms, linewidth=0.8, label="Sparse Trie Wait", color="#9467bd")
    ax.set_xlabel("Block Number")
    ax.set_ylabel("Wait Time (ms)")
    ax.set_title("Wait Time Breakdown per Block")
    ax.legend()
    ax.grid(True, alpha=0.3)
    fig.tight_layout()
    fig.savefig(out, dpi=150)
    plt.close(fig)


def plot_gas_vs_latency(rows: list[dict], out: Path):
    gas_mgas = [r["gas_used"] / 1_000_000 for r in rows]
    latency_ms = [r["new_payload_latency_us"] / 1_000 for r in rows]

    fig, ax = plt.subplots(figsize=(8, 6))
    ax.scatter(gas_mgas, latency_ms, s=8, alpha=0.6, color="#1f77b4")
    ax.set_xlabel("Gas Used (Mgas)")
    ax.set_ylabel("newPayload Latency (ms)")
    ax.set_title("Gas Used vs Latency")
    ax.grid(True, alpha=0.3)
    fig.tight_layout()
    fig.savefig(out, dpi=150)
    plt.close(fig)


def main():
    parser = argparse.ArgumentParser(description="Generate benchmark charts")
    parser.add_argument("combined_csv", help="Path to combined_latency.csv")
    parser.add_argument("--output-dir", required=True, help="Output directory for PNG charts")
    args = parser.parse_args()

    rows = parse_combined_csv(args.combined_csv)
    if not rows:
        print("No results found in combined CSV", file=sys.stderr)
        sys.exit(1)

    out_dir = Path(args.output_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    plot_latency(rows, out_dir / "newpayload_latency.png")
    plot_gas_per_second(rows, out_dir / "gas_per_second.png")
    plot_wait_breakdown(rows, out_dir / "wait_breakdown.png")
    plot_gas_vs_latency(rows, out_dir / "gas_vs_latency.png")

    print(f"Charts written to {out_dir}")


if __name__ == "__main__":
    main()
