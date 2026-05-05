#!/usr/bin/env bash
set -euo pipefail

CHART_ARGS="--output-dir $BENCH_WORK_DIR/charts"
FEATURE_CSVS="$BENCH_WORK_DIR/feature-1/combined_latency.csv"
BASELINE_CSVS="$BENCH_WORK_DIR/baseline-1/combined_latency.csv"
if [ "${BENCH_ABBA:-true}" = "true" ]; then
  FEATURE_CSVS="$FEATURE_CSVS $BENCH_WORK_DIR/feature-2/combined_latency.csv"
  BASELINE_CSVS="$BASELINE_CSVS $BENCH_WORK_DIR/baseline-2/combined_latency.csv"
fi
CHART_ARGS="$CHART_ARGS --feature $FEATURE_CSVS"
CHART_ARGS="$CHART_ARGS --baseline $BASELINE_CSVS"
CHART_ARGS="$CHART_ARGS --baseline-name ${BASELINE_NAME}"
CHART_ARGS="$CHART_ARGS --feature-name ${FEATURE_NAME}"

# shellcheck disable=SC2086
uv run --with matplotlib python3 .github/scripts/bench-reth-charts.py $CHART_ARGS
