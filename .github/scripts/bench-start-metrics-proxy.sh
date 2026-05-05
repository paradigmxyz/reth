#!/usr/bin/env bash
set -euo pipefail

BENCH_ID="ci-${GITHUB_RUN_ID}"
BENCH_REFERENCE_EPOCH=$(date +%s)
echo "BENCH_ID=${BENCH_ID}" >> "$GITHUB_ENV"
echo "BENCH_REFERENCE_EPOCH=${BENCH_REFERENCE_EPOCH}" >> "$GITHUB_ENV"

LABELS_FILE="/tmp/bench-metrics-labels.json"
echo '{}' > "$LABELS_FILE"
echo "BENCH_LABELS_FILE=${LABELS_FILE}" >> "$GITHUB_ENV"

python3 .github/scripts/bench-metrics-proxy.py \
  --labels "$LABELS_FILE" \
  --upstream "http://${BENCH_METRICS_ADDR}/" \
  --subnet 10.10.0.0/24 \
  --port 9090 &
PROXY_PID=$!
echo "BENCH_METRICS_PROXY_PID=${PROXY_PID}" >> "$GITHUB_ENV"
echo "Metrics proxy started (PID $PROXY_PID)"
