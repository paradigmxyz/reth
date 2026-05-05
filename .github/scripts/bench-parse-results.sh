#!/usr/bin/env bash
set -euo pipefail

git fetch origin "${BASELINE_NAME}" --quiet 2>/dev/null || true
BASELINE_HEAD=$(git rev-parse "origin/${BASELINE_NAME}" 2>/dev/null || echo "")
BEHIND_BASELINE=0
if [ -n "$BASELINE_HEAD" ] && [ "$BASELINE_REF" != "$BASELINE_HEAD" ]; then
  BEHIND_BASELINE=$(git rev-list --count "${BASELINE_REF}..${BASELINE_HEAD}" 2>/dev/null || echo "0")
fi

SUMMARY_ARGS="--output-summary $BENCH_WORK_DIR/summary.json"
SUMMARY_ARGS="$SUMMARY_ARGS --output-markdown $BENCH_WORK_DIR/comment.md"
SUMMARY_ARGS="$SUMMARY_ARGS --repo ${GITHUB_REPOSITORY}"
SUMMARY_ARGS="$SUMMARY_ARGS --baseline-ref ${BASELINE_REF}"
SUMMARY_ARGS="$SUMMARY_ARGS --baseline-name ${BASELINE_NAME}"
SUMMARY_ARGS="$SUMMARY_ARGS --feature-name ${FEATURE_NAME}"
SUMMARY_ARGS="$SUMMARY_ARGS --feature-ref ${FEATURE_REF}"
BASELINE_CSVS="$BENCH_WORK_DIR/baseline-1/combined_latency.csv"
FEATURE_CSVS="$BENCH_WORK_DIR/feature-1/combined_latency.csv"
if [ "${BENCH_ABBA:-true}" = "true" ]; then
  BASELINE_CSVS="$BASELINE_CSVS $BENCH_WORK_DIR/baseline-2/combined_latency.csv"
  FEATURE_CSVS="$FEATURE_CSVS $BENCH_WORK_DIR/feature-2/combined_latency.csv"
fi
SUMMARY_ARGS="$SUMMARY_ARGS --baseline-csv $BASELINE_CSVS"
SUMMARY_ARGS="$SUMMARY_ARGS --feature-csv $FEATURE_CSVS"
SUMMARY_ARGS="$SUMMARY_ARGS --gas-csv $BENCH_WORK_DIR/feature-1/total_gas.csv"
if [ "$BEHIND_BASELINE" -gt 0 ]; then
  SUMMARY_ARGS="$SUMMARY_ARGS --behind-baseline $BEHIND_BASELINE"
fi
if [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
  SUMMARY_ARGS="$SUMMARY_ARGS --big-blocks"
fi
if [ -n "${BENCH_WARMUP_BLOCKS:-}" ]; then
  SUMMARY_ARGS="$SUMMARY_ARGS --warmup-blocks $BENCH_WARMUP_BLOCKS"
fi
if [ -n "${BENCH_WAIT_TIME:-}" ]; then
  SUMMARY_ARGS="$SUMMARY_ARGS --wait-time $BENCH_WAIT_TIME"
fi
if [ -n "${BENCH_BAL:-}" ] && [ "${BENCH_BAL}" != "false" ]; then
  SUMMARY_ARGS="$SUMMARY_ARGS --bal-mode $BENCH_BAL"
fi
if [ -n "${BENCH_DRIVER:-}" ]; then
  SUMMARY_ARGS="$SUMMARY_ARGS --driver $BENCH_DRIVER"
fi
if [ -n "${GRAFANA_URL:-}" ]; then
  SUMMARY_ARGS="$SUMMARY_ARGS --grafana-url $GRAFANA_URL"
fi

# shellcheck disable=SC2086
python3 .github/scripts/bench-reth-summary.py $SUMMARY_ARGS
