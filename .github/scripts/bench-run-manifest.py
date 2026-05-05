#!/usr/bin/env python3
"""Write a small manifest for one benchmark run."""

import argparse
import json
import os
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--run-dir", required=True)
    parser.add_argument("--run-type", required=True)
    parser.add_argument("--git-ref", required=True)
    parser.add_argument("--node-binary", required=True)
    parser.add_argument("--status", required=True)
    args = parser.parse_args()

    output_dir = Path(args.output_dir)
    txgen_dir = output_dir / "txgen"
    driver = os.environ.get("BENCH_DRIVER", "")

    reports = {
        "node_log": str(output_dir / "node.log"),
        "reth_logs_dir": str(output_dir / "reth-logs"),
        "combined_latency_csv": str(output_dir / "combined_latency.csv"),
        "total_gas_csv": str(output_dir / "total_gas.csv"),
        "tracy_profile": str(output_dir / "tracy-profile.tracy"),
        "samply_profile": str(output_dir / "samply-profile.json.gz"),
    }
    if driver == "txgen":
        reports.update({
            "txgen_report_json": str(output_dir / "report.json"),
            "txgen_warmup_report_json": str(txgen_dir / "warmup-report.json"),
            "txgen_all_blocks": str(txgen_dir / "all-blocks.ndjson"),
            "txgen_warmup_blocks": str(txgen_dir / "warmup-blocks.ndjson"),
            "txgen_benchmark_blocks": str(txgen_dir / "benchmark-blocks.ndjson"),
        })

    manifest = {
        "driver": driver,
        "status": args.status,
        "run_dir": args.run_dir,
        "run_type": args.run_type,
        "git_ref": args.git_ref,
        "node_binary": args.node_binary,
        "node_bin": os.environ.get("BENCH_NODE_BIN", ""),
        "output_dir": str(output_dir),
        "work_dir": os.environ.get("BENCH_WORK_DIR", ""),
        "benchmark_id": os.environ.get("BENCH_ID", ""),
        "run_start_epoch": os.environ.get("BENCH_LAST_RUN_START", ""),
        "reference_epoch": os.environ.get("BENCH_REFERENCE_EPOCH", ""),
        "reports": reports,
    }

    output_dir.mkdir(parents=True, exist_ok=True)
    (output_dir / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
