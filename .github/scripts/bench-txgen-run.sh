#!/usr/bin/env bash
#
# Runs a single txgen-backed Engine API benchmark cycle:
# mount snapshot → start node → extract source blocks → warmup → send-blocks →
# convert txgen JSON report into the legacy reth-bench CSVs.
#
# Usage: bench-txgen-run.sh <label> <binary> <output-dir>
#
# Required env: SCHELK_MOUNT, BENCH_RPC_URL, BENCH_BLOCKS, BENCH_WARMUP_BLOCKS
# Optional env: BENCH_WORK_DIR, BENCH_BASELINE_ARGS, BENCH_FEATURE_ARGS,
#               BENCH_OTLP_TRACES_ENDPOINT, BENCH_OTLP_LOGS_ENDPOINT,
#               BENCH_OTLP_DISABLED, BENCH_TRACY, BENCH_TRACY_FILTER,
#               BENCH_TRACY_SAMPLING_HZ
set -euxo pipefail

LABEL="$1"
BINARY="$2"
OUTPUT_DIR="$3"
DATADIR="$SCHELK_MOUNT/datadir"

# Unsupported txgen-path behavior is made explicit here. Keep these checks near
# the top so new txgen support can be added one feature at a time.
if [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
  echo "::error::txgen driver does not support big-block benchmarks yet; use the reth-bench driver"
  exit 1
fi
if [ -n "${BENCH_WAIT_TIME:-}" ]; then
  echo "::error::txgen driver does not support BENCH_WAIT_TIME yet"
  exit 1
fi
if [ -n "${BENCH_BAL:-}" ] && [ "${BENCH_BAL}" != "false" ]; then
  echo "::error::txgen driver does not support BAL replay yet; use big-blocks with the reth-bench driver"
  exit 1
fi

# shellcheck source=.github/scripts/bench-node-lifecycle.sh
source .github/scripts/bench-node-lifecycle.sh
bench_init_node_lifecycle "$LABEL" "$BINARY" "$OUTPUT_DIR" "$DATADIR"
bench_run_node_lifecycle

TXGEN_ETHEREUM="$(which txgen-ethereum)"
TXGEN_BENCH="$(which bench)"
BENCH_NICE="sudo nice -n -20 sudo -u $(id -un)"

HEAD_JSON=$(curl -sf http://127.0.0.1:8545 -X POST \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}')
HEAD_HEX=$(jq -r '.result' <<< "$HEAD_JSON")
HEAD_DEC=$((16#${HEAD_HEX#0x}))

WARMUP="${BENCH_WARMUP_BLOCKS:-0}"
BLOCKS="${BENCH_BLOCKS:?BENCH_BLOCKS must be set}"
TOTAL=$(( WARMUP + BLOCKS ))
if [ "$BLOCKS" -le 0 ] || [ "$TOTAL" -le 0 ]; then
  echo "::error::BENCH_BLOCKS must be greater than 0"
  exit 1
fi

TXGEN_DIR="$OUTPUT_DIR/txgen"
mkdir -p "$TXGEN_DIR"
ALL_BLOCKS="$TXGEN_DIR/all-blocks.ndjson"
WARMUP_BLOCKS="$TXGEN_DIR/warmup-blocks.ndjson"
BENCHMARK_BLOCKS="$TXGEN_DIR/benchmark-blocks.ndjson"

EXTRACT_FROM=$(( HEAD_DEC + 1 ))
EXTRACT_TO=$(( HEAD_DEC + TOTAL ))
SOURCE_RPC_URL="${BENCH_TXGEN_RPC_URL:-$BENCH_RPC_URL}"
echo "Extracting blocks ${EXTRACT_FROM}..${EXTRACT_TO} for txgen benchmark (${WARMUP} warmup, ${BLOCKS} measured)"
python3 .github/scripts/bench-txgen-extract.py \
  --rpc "$SOURCE_RPC_URL" \
  --metadata-rpc "$BENCH_RPC_URL" \
  --from "$EXTRACT_FROM" \
  --to "$EXTRACT_TO" \
  -o "$ALL_BLOCKS"

if [ "$WARMUP" -gt 0 ] 2>/dev/null; then
  head -n "$WARMUP" "$ALL_BLOCKS" > "$WARMUP_BLOCKS"
else
  : > "$WARMUP_BLOCKS"
fi
awk -v warmup="$WARMUP" 'NR > warmup { print }' "$ALL_BLOCKS" > "$BENCHMARK_BLOCKS"

if [ "$WARMUP" -gt 0 ] 2>/dev/null; then
  echo "Running txgen warmup (${WARMUP} blocks)..."
  $BENCH_NICE "$TXGEN_BENCH" send-blocks \
    --engine http://127.0.0.1:8551 \
    --jwt-secret "$DATADIR/jwt.hex" \
    --input "$WARMUP_BLOCKS" \
    --wait-for-persistence never \
    --report json:"$TXGEN_DIR/warmup-report.json" 2>&1 | sed -u "s/^/[bench] /"
else
  echo "Skipping warmup (0 blocks)..."
fi

bench_start_tracy_capture

# TODO(txgen): expose a wait-time flag and plumb BENCH_WAIT_TIME here.
# TODO(txgen): expose microsecond client-side FCU latency to avoid ms rounding.
# TODO(txgen): support reth-bb payload/env-switch/BAL replay so big-blocks can move here.
echo "Running txgen measured benchmark (${BLOCKS} blocks)..."
$BENCH_NICE "$TXGEN_BENCH" send-blocks \
  --engine http://127.0.0.1:8551 \
  --jwt-secret "$DATADIR/jwt.hex" \
  --input "$BENCHMARK_BLOCKS" \
  --wait-for-persistence never \
  --report json:"$OUTPUT_DIR/report.json" 2>&1 | sed -u "s/^/[bench] /"

python3 .github/scripts/bench-txgen-report-to-reth-csv.py "$OUTPUT_DIR/report.json" "$OUTPUT_DIR"
