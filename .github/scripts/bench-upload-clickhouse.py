#!/usr/bin/env python3
"""Upload bench-scheduled summary.json results to ClickHouse.

Reads the summary JSON produced by bench-reth-summary.py and inserts a row
into the bench_dual_comparisons table so the PM dashboard can display results.

Usage:
    bench-upload-clickhouse.py \
        --summary <summary.json> \
        --workflow-name <name> \
        --chain <chain>

Environment variables:
    CLICKHOUSE_HOST      ClickHouse host URL
    CLICKHOUSE_USER      ClickHouse username
    CLICKHOUSE_PASSWORD  ClickHouse password
    CLICKHOUSE_DATABASE  ClickHouse database (default: "default")
"""

import argparse
import json
import os
import sys
import urllib.request
import urllib.error


def main():
    parser = argparse.ArgumentParser(description="Upload benchmark results to ClickHouse")
    parser.add_argument("--summary", required=True, help="Path to summary.json")
    parser.add_argument("--workflow-name", required=True, help="Workflow name for ClickHouse")
    parser.add_argument("--chain", default="mainnet", help="Chain name")
    parser.add_argument("--grafana-url", default="", help="Grafana dashboard URL")
    parser.add_argument("--github-diff-url", default="", help="GitHub diff URL")
    parser.add_argument("--job-url", default="", help="CI job URL")
    args = parser.parse_args()

    ch_host = os.environ.get("CLICKHOUSE_HOST", "")
    ch_user = os.environ.get("CLICKHOUSE_USER", "")
    ch_password = os.environ.get("CLICKHOUSE_PASSWORD", "")
    ch_database = os.environ.get("CLICKHOUSE_DATABASE", "default")
    ch_table = "bench_dual_comparisons"

    if not ch_host or not ch_user or not ch_password:
        print("Missing ClickHouse credentials, skipping upload", file=sys.stderr)
        sys.exit(0)

    with open(args.summary) as f:
        summary = json.load(f)

    baseline = summary["baseline"]
    feature = summary["feature"]
    b_stats = baseline["stats"]
    f_stats = feature["stats"]
    changes = summary["changes"]
    blocks = summary["blocks"]

    # Extract wait time data
    wait_times = summary.get("wait_times", {})
    def wait_mean(field):
        wt = wait_times.get(field, {})
        b = wt.get("baseline", {}).get("mean_ms", 0.0)
        f = wt.get("feature", {}).get("mean_ms", 0.0)
        return b, f

    b_persist, f_persist = wait_mean("persistence_wait_us")
    b_exec_cache, f_exec_cache = wait_mean("execution_cache_wait_us")
    b_sparse, f_sparse = wait_mean("sparse_trie_wait_us")

    # gas_per_second: summary uses mean_mgas_s (Mgas/s), ClickHouse stores gas/s
    b_gas_per_second = b_stats["mean_mgas_s"] * 1_000_000
    f_gas_per_second = f_stats["mean_mgas_s"] * 1_000_000

    mean_change = changes.get("mean", {}).get("pct", 0.0)
    gas_change = changes.get("mgas_s", {}).get("pct", 0.0)
    latency_improved = 1 if mean_change < 0 else 0
    throughput_improved = 1 if gas_change > 0 else 0

    big_blocks = "true" if summary.get("big_blocks", False) else "false"
    warmup_blocks = summary.get("warmup_blocks", 0) or 0

    def esc(s):
        return str(s).replace("'", "\\'")

    insert = f"""
    INSERT INTO {ch_database}.{ch_table} (
        workflow_name, chain,
        baseline_ref, baseline_commit,
        feature_ref, feature_commit,
        blocks,
        baseline_total_latency_ms, baseline_gas_per_second,
        baseline_latency_mean_ms, baseline_latency_median_ms,
        baseline_latency_p90_ms, baseline_latency_p99_ms,
        feature_total_latency_ms, feature_gas_per_second,
        feature_latency_mean_ms, feature_latency_median_ms,
        feature_latency_p90_ms, feature_latency_p99_ms,
        mean_latency_change_percent, gas_per_second_change_percent,
        latency_improved, throughput_improved,
        warmup_blocks, big_blocks,
        grafana_benchmark_url, github_diff_url, argo_workflow_url,
        baseline_persistence_wait_mean_ms, baseline_execution_cache_wait_mean_ms,
        baseline_sparse_trie_wait_mean_ms,
        feature_persistence_wait_mean_ms, feature_execution_cache_wait_mean_ms,
        feature_sparse_trie_wait_mean_ms
    ) VALUES (
        '{esc(args.workflow_name)}', '{esc(args.chain)}',
        '{esc(baseline["ref"])}', '{esc(baseline["ref"])}',
        '{esc(feature["ref"])}', '{esc(feature["ref"])}',
        {blocks},
        {b_stats.get("wall_clock_s", 0) * 1000}, {b_gas_per_second},
        {b_stats["mean_ms"]}, {b_stats["p50_ms"]},
        {b_stats["p90_ms"]}, {b_stats["p99_ms"]},
        {f_stats.get("wall_clock_s", 0) * 1000}, {f_gas_per_second},
        {f_stats["mean_ms"]}, {f_stats["p50_ms"]},
        {f_stats["p90_ms"]}, {f_stats["p99_ms"]},
        {mean_change}, {gas_change},
        {latency_improved}, {throughput_improved},
        {warmup_blocks}, '{big_blocks}',
        '{esc(args.grafana_url)}', '{esc(args.github_diff_url)}', '{esc(args.job_url)}',
        {b_persist}, {b_exec_cache}, {b_sparse},
        {f_persist}, {f_exec_cache}, {f_sparse}
    );
    """

    # Build ClickHouse HTTP URL (credentials via headers, never in URL)
    host = ch_host.rstrip("/")
    if not host.startswith("http"):
        host = f"https://{host}:8443"

    url = f"{host}/?database={ch_database}"

    req = urllib.request.Request(url, data=insert.encode("utf-8"), method="POST")
    req.add_header("Content-Type", "text/plain")
    req.add_header("X-ClickHouse-User", ch_user)
    req.add_header("X-ClickHouse-Key", ch_password)

    try:
        with urllib.request.urlopen(req) as resp:
            body = resp.read().decode("utf-8")
            if body.strip():
                print(f"ClickHouse response: {body}")
        print(f"Successfully uploaded benchmark results to ClickHouse ({args.workflow_name})")
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8")
        print(f"ClickHouse upload failed ({e.code}): {body}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
