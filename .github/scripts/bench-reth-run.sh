#!/usr/bin/env bash
#
# Runs a single reth-bench cycle: mount snapshot → start node → warmup →
# benchmark → stop node → recover snapshot.
#
# Usage: bench-reth-run.sh <label> <binary> <output-dir>
#
# Required env: SCHELK_MOUNT, BENCH_RPC_URL, BENCH_BLOCKS, BENCH_WARMUP_BLOCKS
# Optional env: BENCH_BIG_BLOCKS (true/false), BENCH_WORK_DIR (for big blocks path)
#               BENCH_BAL (false/true/feature/baseline; only used with big blocks)
#               BENCH_WAIT_TIME (duration like 500ms, default empty)
#               BENCH_BASELINE_ARGS (extra reth node args for baseline runs)
#               BENCH_FEATURE_ARGS (extra reth node args for feature runs)
#               BENCH_OTLP_TRACES_ENDPOINT (OTLP HTTP endpoint for traces, e.g. https://host/insert/opentelemetry/v1/traces)
#               BENCH_OTLP_LOGS_ENDPOINT (OTLP HTTP endpoint for logs, e.g. https://host/insert/opentelemetry/v1/logs)
#               BENCH_OTLP_DISABLED (true to skip OTLP export even if endpoints are set)
set -euxo pipefail

LABEL="$1"
BINARY="$2"
OUTPUT_DIR="$3"
DATADIR_NAME="datadir"
if [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
  DATADIR_NAME="datadir-big-blocks"
fi
DATADIR="$SCHELK_MOUNT/$DATADIR_NAME"

# shellcheck source=.github/scripts/bench-node-lifecycle.sh
source .github/scripts/bench-node-lifecycle.sh
bench_init_node_lifecycle "$LABEL" "$BINARY" "$OUTPUT_DIR" "$DATADIR"
bench_run_node_lifecycle

RETH_BENCH="$(which reth-bench)"
BIG_BLOCKS="${BENCH_BIG_BLOCKS:-false}"

# Run reth-bench with high priority but as the current user so output
# files are not root-owned (avoids EACCES on next checkout).
BENCH_NICE="sudo nice -n -20 sudo -u $(id -un)"

# Build optional flags
EXTRA_BENCH_ARGS=(--reth-new-payload)
if [ -n "${BENCH_WAIT_TIME:-}" ]; then
  EXTRA_BENCH_ARGS+=(--wait-time "$BENCH_WAIT_TIME")
fi

if [ "$BIG_BLOCKS" = "true" ]; then
  # Big blocks mode: replay pre-generated payloads
  BIG_BLOCKS_DIR="${BENCH_BIG_BLOCKS_DIR:-${BENCH_WORK_DIR}/big-blocks}"
  BENCH_BAL_MODE="${BENCH_BAL:-false}"

  BB_BENCH_ARGS=(--reth-new-payload)
  if [ -n "${BENCH_WAIT_TIME:-}" ]; then
    BB_BENCH_ARGS+=(--wait-time "$BENCH_WAIT_TIME")
  fi
  case "$BENCH_BAL_MODE" in
    false)
      ;;
    true)
      BB_BENCH_ARGS+=(--bal)
      ;;
    baseline)
      if [[ "$LABEL" == baseline* ]]; then
        BB_BENCH_ARGS+=(--bal)
      fi
      ;;
    feature)
      if [[ "$LABEL" == feature* ]]; then
        BB_BENCH_ARGS+=(--bal)
      fi
      ;;
    *)
      echo "::error::Unknown BENCH_BAL value: $BENCH_BAL_MODE"
      exit 1
      ;;
  esac

  # Warmup
  WARMUP="${BENCH_WARMUP_BLOCKS:-50}"
  if [ "$WARMUP" -gt 0 ] 2>/dev/null; then
    echo "Running big blocks warmup (${WARMUP} payloads)..."
    $BENCH_NICE "$RETH_BENCH" replay-payloads \
      "${BB_BENCH_ARGS[@]}" \
      --count "$WARMUP" \
      --payload-dir "$BIG_BLOCKS_DIR/payloads" \
      --engine-rpc-url http://127.0.0.1:8551 \
      --jwt-secret "$DATADIR/jwt.hex" 2>&1 | sed -u "s/^/[bench] /"
  fi

  # Start tracy-capture after warmup so profile only covers the benchmark
  bench_start_tracy_capture

  # Benchmark — skip warmup payloads so they aren't measured
  BB_SKIP=0
  if [ "$WARMUP" -gt 0 ] 2>/dev/null; then
    BB_SKIP="$WARMUP"
  fi
  if [ "${BENCH_BLOCKS:-0}" -gt 0 ] 2>/dev/null; then
    BB_BENCH_ARGS+=(--count "$BENCH_BLOCKS")
  fi

  echo "Running big blocks benchmark (replay-payloads, skip=${BB_SKIP})..."
  $BENCH_NICE "$RETH_BENCH" replay-payloads \
    "${BB_BENCH_ARGS[@]}" \
    --skip "$BB_SKIP" \
    --payload-dir "$BIG_BLOCKS_DIR/payloads" \
    --engine-rpc-url http://127.0.0.1:8551 \
    --jwt-secret "$DATADIR/jwt.hex" \
    --output "$OUTPUT_DIR" 2>&1 | sed -u "s/^/[bench] /"
else
  # Standard mode: warmup + new-payload-fcu
  WARMUP="${BENCH_WARMUP_BLOCKS:-50}"
  if [ "$WARMUP" -gt 0 ] 2>/dev/null; then
    # Warm up the node before measuring the benchmark window.
    $BENCH_NICE "$RETH_BENCH" new-payload-fcu \
      --rpc-url "$BENCH_RPC_URL" \
      --engine-rpc-url http://127.0.0.1:8551 \
      --jwt-secret "$DATADIR/jwt.hex" \
      --advance "$WARMUP" \
      "${EXTRA_BENCH_ARGS[@]}" 2>&1 | sed -u "s/^/[bench] /"
  else
    echo "Skipping warmup (0 blocks)..."
  fi

  # Start tracy-capture after warmup so profile only covers the benchmark
  bench_start_tracy_capture

  # Benchmark
  $BENCH_NICE "$RETH_BENCH" new-payload-fcu \
    --rpc-url "$BENCH_RPC_URL" \
    --engine-rpc-url http://127.0.0.1:8551 \
    --jwt-secret "$DATADIR/jwt.hex" \
    --advance "$BENCH_BLOCKS" \
    "${EXTRA_BENCH_ARGS[@]}" \
    --output "$OUTPUT_DIR" 2>&1 | sed -u "s/^/[bench] /"
fi

# cleanup runs via trap
