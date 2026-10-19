#!/usr/bin/env python3
"""Convert txgen `bench send-blocks` or `bench send` JSON into legacy benchmark CSVs.

The benchmark rendering pipeline still consumes `combined_latency.csv` and
`total_gas.csv`. This adapter lets the txgen runner reuse the existing
summary/charts/slack code while we migrate those consumers to txgen JSON.
"""

import argparse
import csv
import json
from pathlib import Path


def opt_int(value, default=None):
    if value is None:
        return default
    return int(value)


def block_latency_us(block: dict, fallback_block_time_ms: int = 0) -> tuple[int, int, int]:
    # txgen currently records server newPayload latency in microseconds but
    # client-side forkchoiceUpdated latency in milliseconds.
    new_payload_us = opt_int(block.get("new_payload_server_latency_us"))
    if new_payload_us is None:
        new_payload_ms = opt_int(block.get("new_payload_ms"))
        if new_payload_ms is None:
            # `bench send` has no Engine API timings. Use locally mined block
            # time so the existing gas/latency summaries remain meaningful.
            new_payload_ms = opt_int(block.get("block_time_ms"), fallback_block_time_ms)
        new_payload_us = new_payload_ms * 1000
    fcu_us = opt_int(block.get("forkchoice_updated_ms"), 0) * 1000
    return new_payload_us, fcu_us, new_payload_us + fcu_us


def main() -> None:
    parser = argparse.ArgumentParser(description="Convert txgen JSON report to benchmark CSVs")
    parser.add_argument("report", help="txgen JSON report path")
    parser.add_argument("output_dir", help="directory for combined_latency.csv and total_gas.csv")
    args = parser.parse_args()

    report_path = Path(args.report)
    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    with report_path.open() as f:
        report = json.load(f)

    blocks = report.get("blocks") or []
    if not blocks:
        raise SystemExit(f"txgen report {report_path} does not contain any blocks")

    fallback_block_time_ms = next(
        (int(block["block_time_ms"]) for block in blocks if block.get("block_time_ms") is not None),
        0,
    )

    combined_path = output_dir / "combined_latency.csv"
    with combined_path.open("w", newline="") as f:
        writer = csv.DictWriter(
            f,
            fieldnames=[
                "block_number",
                "gas_limit",
                "transaction_count",
                "gas_used",
                "new_payload_latency",
                "fcu_latency",
                "total_latency",
                "persistence_wait",
                "execution_cache_wait",
                "sparse_trie_wait",
            ],
        )
        writer.writeheader()
        for block in blocks:
            new_payload_us, fcu_us, total_us = block_latency_us(block, fallback_block_time_ms)
            writer.writerow(
                {
                    "block_number": block["number"],
                    "gas_limit": block["gas_limit"],
                    "transaction_count": block["tx_count"],
                    "gas_used": block["gas_used"],
                    "new_payload_latency": new_payload_us,
                    "fcu_latency": fcu_us,
                    "total_latency": total_us,
                    "persistence_wait": block.get("persistence_wait_us") or 0,
                    "execution_cache_wait": block.get("execution_cache_wait_us") or 0,
                    "sparse_trie_wait": block.get("sparse_trie_wait_us") or 0,
                }
            )

    total_gas_path = output_dir / "total_gas.csv"
    elapsed_us = 0
    with total_gas_path.open("w", newline="") as f:
        writer = csv.DictWriter(
            f,
            fieldnames=["block_number", "transaction_count", "gas_used", "time"],
        )
        writer.writeheader()
        for block in blocks:
            _, _, total_us = block_latency_us(block, fallback_block_time_ms)
            elapsed_us += total_us
            writer.writerow(
                {
                    "block_number": block["number"],
                    "transaction_count": block["tx_count"],
                    "gas_used": block["gas_used"],
                    "time": elapsed_us,
                }
            )

    print(f"Wrote legacy CSVs from {report_path} to {output_dir}")


if __name__ == "__main__":
    main()
