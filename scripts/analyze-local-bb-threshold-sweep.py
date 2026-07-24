#!/usr/bin/env python3
from __future__ import annotations

import csv
import json
from pathlib import Path
import sys


def percentile(values: list[float], pct: int) -> float:
    if not values:
        return 0.0
    sorted_values = sorted(values)
    idx = int(len(sorted_values) * pct / 100)
    return sorted_values[min(idx, len(sorted_values) - 1)]


def pct(base: float, value: float) -> float:
    return (value - base) / base * 100.0 if base else 0.0


def parse_combined_latency(path: Path) -> dict:
    rows = []
    with path.open() as f:
        rows.extend(csv.DictReader(f))

    total_latency_us = 0
    persistence_wait_us = 0
    waits_ms = []
    throughputs = []

    for row in rows:
        gas_used = int(row["gas_used"])
        new_payload_us = int(row["new_payload_latency"])
        total_us = int(row["total_latency"])
        wait_us = int(row.get("persistence_wait") or 0)

        total_latency_us += total_us
        persistence_wait_us += wait_us
        waits_ms.append(wait_us / 1_000)
        if new_payload_us > 0:
            throughputs.append(gas_used / (new_payload_us / 1_000_000) / 1_000_000)

    n = len(rows)
    return {
        "blocks": n,
        "persist_mean_ms": persistence_wait_us / n / 1_000 if n else 0.0,
        "persist_p50_ms": percentile(waits_ms, 50),
        "persist_p95_ms": percentile(waits_ms, 95),
        "wall_excluding_persist_s": total_latency_us / 1_000_000,
        "wall_including_persist_s": (total_latency_us + persistence_wait_us) / 1_000_000,
        "throughput_mean_mgas_s": sum(throughputs) / len(throughputs) if throughputs else 0.0,
        "throughput_p50_mgas_s": percentile(throughputs, 50),
        "throughput_p90_mgas_s": percentile(throughputs, 90),
        "throughput_p99_mgas_s": percentile(throughputs, 99),
    }


def load_grouped_scrapes(run_dir: Path) -> list[dict]:
    samples_path = run_dir / "target-metrics-scrapes.jsonl"
    grouped: dict[int, dict] = {}
    if not samples_path.exists():
        return []

    with samples_path.open() as f:
        for line in f:
            if not line.strip():
                continue
            sample = json.loads(line)
            unix_ms = int(sample["unix_ms"])
            scrape = grouped.setdefault(
                unix_ms,
                {"unix_ms": unix_ms, "offset_ms": int(sample["offset_ms"]), "samples": []},
            )
            scrape["samples"].append(sample)

    return [grouped[k] for k in sorted(grouped)]


def sample_sum(scrape: dict, metric_name: str) -> float:
    return sum(float(sample["value"]) for sample in scrape["samples"] if sample["name"] == metric_name)


def sample_single(scrape: dict, metric_name: str) -> float | None:
    values = [float(sample["value"]) for sample in scrape["samples"] if sample["name"] == metric_name]
    if not values:
        return None
    return values[0] if len(values) == 1 else sum(values)


def save_blocks_mdbx_mean_ms(run_dir: Path) -> float:
    range_path = run_dir / "target-metrics-range.json"
    if not range_path.exists():
        return 0.0

    with range_path.open() as f:
        range_meta = json.load(f)
    start_ms = int(range_meta["range_start_ms"])
    end_ms = int(range_meta["range_end_ms"])

    scrapes = [
        scrape
        for scrape in load_grouped_scrapes(run_dir)
        if start_ms <= int(scrape["unix_ms"]) <= end_ms
    ]
    if len(scrapes) < 2:
        return 0.0

    metric_sum = "reth_storage_providers_database_save_blocks_mdbx_sum"
    height_metric = "reth_blockchain_tree_canonical_chain_height"
    total_sum_delta = 0.0
    total_block_delta = 0.0

    previous_sum = sample_sum(scrapes[0], metric_sum)
    previous_height = sample_single(scrapes[0], height_metric)
    if previous_height is None:
        return 0.0

    for scrape in scrapes[1:]:
        current_sum = sample_sum(scrape, metric_sum)
        current_height = sample_single(scrape, height_metric)
        if current_height is None:
            continue
        block_delta = current_height - previous_height
        sum_delta = current_sum - previous_sum
        if block_delta > 0 and sum_delta >= 0:
            total_block_delta += block_delta
            total_sum_delta += sum_delta
        previous_sum = current_sum
        previous_height = current_height

    return total_sum_delta / total_block_delta * 1_000 if total_block_delta else 0.0


def threshold_from_dir(run_dir: Path) -> int:
    metadata_path = run_dir / "metadata.json"
    if metadata_path.exists():
        with metadata_path.open() as f:
            return int(json.load(f)["threshold"])
    return int(run_dir.name.rsplit("-", 1)[1])


def collect(root: Path) -> list[dict]:
    rows = []
    for run_dir in sorted(root.glob("threshold-*")):
        csv_path = run_dir / "combined_latency.csv"
        if not csv_path.exists():
            continue
        row = parse_combined_latency(csv_path)
        row["threshold"] = threshold_from_dir(run_dir)
        row["save_blocks_mdbx_mean_ms"] = save_blocks_mdbx_mean_ms(run_dir)
        rows.append(row)
    return sorted(rows, key=lambda row: row["threshold"])


def write_json(root: Path, rows: list[dict]) -> None:
    with (root / "analysis.json").open("w") as f:
        json.dump(rows, f, indent=2)
        f.write("\n")


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: analyze-local-bb-threshold-sweep.py <artifact-root>", file=sys.stderr)
        return 2

    root = Path(sys.argv[1])
    rows = collect(root)
    if not rows:
        print(f"no completed threshold runs found under {root}", file=sys.stderr)
        return 1

    write_json(root, rows)

    print("| threshold | persist mean | save_blocks mdbx mean | persist p95 | wall + persist | validation throughput mean/p50/p90/p99 Mgas/s |")
    print("|---:|---:|---:|---:|---:|---:|")
    for row in rows:
        print(
            f"| `{row['threshold']}` "
            f"| `{row['persist_mean_ms']:.2f}ms` "
            f"| `{row['save_blocks_mdbx_mean_ms']:.2f}ms` "
            f"| `{row['persist_p95_ms']:.2f}ms` "
            f"| `{row['wall_including_persist_s']:.2f}s` "
            f"| `{row['throughput_mean_mgas_s']:.0f}/{row['throughput_p50_mgas_s']:.0f}/{row['throughput_p90_mgas_s']:.0f}/{row['throughput_p99_mgas_s']:.0f}` |"
        )

    if len(rows) > 1:
        print()
        print("| threshold step | save_blocks delta | persist mean delta | wall + persist delta | throughput mean delta |")
        print("|---|---:|---:|---:|---:|")
        for previous, current in zip(rows, rows[1:]):
            print(
                f"| `{previous['threshold']}->{current['threshold']}` "
                f"| `{current['save_blocks_mdbx_mean_ms'] - previous['save_blocks_mdbx_mean_ms']:+.2f}ms` "
                f"| `{current['persist_mean_ms'] - previous['persist_mean_ms']:+.2f}ms` "
                f"| `{current['wall_including_persist_s'] - previous['wall_including_persist_s']:+.2f}s` "
                f"| `{pct(previous['throughput_mean_mgas_s'], current['throughput_mean_mgas_s']):+.1f}%` |"
            )

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
