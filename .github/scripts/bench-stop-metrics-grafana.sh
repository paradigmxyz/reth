#!/usr/bin/env bash
set -euo pipefail

kill "$BENCH_METRICS_PROXY_PID" 2>/dev/null || true

LAST_RUN_DURATION=$(( $(date +%s) - BENCH_LAST_RUN_START ))
FROM_MS=$(( BENCH_REFERENCE_EPOCH * 1000 ))
TO_MS=$(( (BENCH_REFERENCE_EPOCH + LAST_RUN_DURATION) * 1000 ))
GRAFANA_URL="https://tempoxyz.grafana.net/d/reth-bench-ghr/reth-bench-ghr?orgId=1&from=${FROM_MS}&to=${TO_MS}&timezone=browser&var-datasource=ef57fux92e9z4e&var-job=reth-bench&var-benchmark_id=${BENCH_ID}&var-benchmark_run=\$__all"
echo "grafana-url=${GRAFANA_URL}" >> "$GITHUB_OUTPUT"
echo "Grafana URL: ${GRAFANA_URL}"
