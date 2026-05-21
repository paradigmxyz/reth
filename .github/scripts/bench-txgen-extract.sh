#!/usr/bin/env bash
#
# Pre-extracts txgen payloads (normal or big blocks) so they can be reused
# across multiple benchmark runs instead of re-fetching from the remote RPC
# every time.
#
# Starts a throwaway reth node to discover the snapshot's chain tip, extracts
# payloads from BENCH_RPC_URL, then stops the node and recovers the snapshot.
#
# Usage: bench-txgen-extract.sh <binary> <output-dir>
#
# On success, writes:
#   <output-dir>/all-blocks.ndjson   (or all-big-blocks.ndjson)
#   <output-dir>/warmup-blocks.ndjson
#   <output-dir>/benchmark-blocks.ndjson
#
# Required env: SCHELK_MOUNT, BENCH_RPC_URL, BENCH_BLOCKS, BENCH_WARMUP_BLOCKS
# Optional env: BENCH_BIG_BLOCKS, BENCH_BIG_BLOCKS_TARGET_GAS, BENCH_BAL
set -euxo pipefail

BINARY="$1"
OUTPUT_DIR="$2"

BIG_BLOCKS="${BENCH_BIG_BLOCKS:-false}"
BAL_MODE="${BENCH_BAL:-false}"
INCLUDE_BAL=false
if [ "$BAL_MODE" != "false" ] && [ -n "$BAL_MODE" ]; then
  INCLUDE_BAL=true
fi

DATADIR_NAME="datadir"
if [ "$BIG_BLOCKS" = "true" ]; then
  DATADIR_NAME="datadir-big-blocks"
fi
DATADIR="$SCHELK_MOUNT/$DATADIR_NAME"

RETH_SCOPE="${RETH_SCOPE:-reth-bench.scope}"

mkdir -p "$OUTPUT_DIR"

cleanup() {
  sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
  sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
  kill "${TAIL_PID:-}" 2>/dev/null || true
  sudo schelk recover -y --kill || true
}
TAIL_PID=
trap cleanup EXIT

# Mount snapshot
sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
sudo schelk recover -y --kill || sudo schelk full-recover -y || true
sudo schelk mount -y || true
if [ ! -d "$DATADIR/db" ] || [ ! -d "$DATADIR/static_files" ]; then
  echo "::error::Failed to mount benchmark datadir at ${DATADIR}"
  exit 1
fi

# Start a lightweight reth node just to query the chain tip.
RETH_ARGS=(
  node
  --datadir "$DATADIR"
  --http
  --http.port 8545
  --disable-discovery
  --no-persist-peers
)

if "$BINARY" node --help 2>/dev/null | grep -qF -- '--debug.startup-sync-state-idle'; then
  RETH_ARGS+=(--debug.startup-sync-state-idle)
fi

LOG="$OUTPUT_DIR/extract-node.log"
sudo systemd-run --quiet --scope --collect --unit="$RETH_SCOPE" \
  nice -n -20 "$BINARY" "${RETH_ARGS[@]}" \
  > "$LOG" 2>&1 &
stdbuf -oL tail -f "$LOG" | sed -u "s/^/[reth-extract] /" &
TAIL_PID=$!

# Wait for RPC
for i in $(seq 1 60); do
  if curl -sf http://127.0.0.1:8545 -X POST \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
    > /dev/null 2>&1; then
    echo "reth (extract) RPC is up after ${i}s"
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo "::error::reth (extract) failed to start within 60s"
    cat "$LOG"
    exit 1
  fi
  sleep 1
done

HEAD_JSON=$(curl -sf http://127.0.0.1:8545 -X POST \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}')
HEAD_HEX=$(jq -r '.result' <<< "$HEAD_JSON")
HEAD_DEC=$((16#${HEAD_HEX#0x}))
echo "Snapshot chain tip: ${HEAD_DEC}"

# Stop the throwaway node and recover snapshot before extraction.
sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
kill "${TAIL_PID:-}" 2>/dev/null || true
TAIL_PID=
sudo schelk recover -y --kill || true

# --- Extract payloads from the remote RPC ---

TXGEN_ETHEREUM="$(which txgen-ethereum)"
WARMUP="${BENCH_WARMUP_BLOCKS:-0}"
BLOCKS="${BENCH_BLOCKS:?BENCH_BLOCKS must be set}"
TOTAL=$(( WARMUP + BLOCKS ))
if [ "$BLOCKS" -le 0 ] || [ "$TOTAL" -le 0 ]; then
  echo "::error::BENCH_BLOCKS must be greater than 0"
  exit 1
fi

ALL_BLOCKS="$OUTPUT_DIR/all-blocks.ndjson"
WARMUP_FILE="$OUTPUT_DIR/warmup-blocks.ndjson"
BENCHMARK_FILE="$OUTPUT_DIR/benchmark-blocks.ndjson"

if [ "$BIG_BLOCKS" = "true" ]; then
  ALL_BLOCKS="$OUTPUT_DIR/all-big-blocks.ndjson"
  WARMUP_FILE="$OUTPUT_DIR/warmup-big-blocks.ndjson"
  BENCHMARK_FILE="$OUTPUT_DIR/measured-big-blocks.ndjson"
fi

EXTRACT_FROM=$(( HEAD_DEC + 1 ))
TXGEN_EXTRACT_ARGS=()
if [ "$INCLUDE_BAL" = "true" ]; then
  TXGEN_EXTRACT_ARGS+=(--bal)
fi
if [ "$BIG_BLOCKS" = "true" ]; then
  echo "Extracting ${TOTAL} big blocks from ${EXTRACT_FROM} for txgen benchmark (${WARMUP} warmup, ${BLOCKS} measured, bal=${INCLUDE_BAL})"
  "$TXGEN_ETHEREUM" extract-big-blocks \
    --rpc "$BENCH_RPC_URL" \
    --from "$EXTRACT_FROM" \
    --count "$TOTAL" \
    --target-gas "${BENCH_BIG_BLOCKS_TARGET_GAS:-1G}" \
    "${TXGEN_EXTRACT_ARGS[@]}" \
    -o "$ALL_BLOCKS"
else
  EXTRACT_TO=$(( HEAD_DEC + TOTAL ))
  echo "Extracting blocks ${EXTRACT_FROM}..${EXTRACT_TO} for txgen benchmark (${WARMUP} warmup, ${BLOCKS} measured, bal=${INCLUDE_BAL})"
  "$TXGEN_ETHEREUM" extract \
    --rpc "$BENCH_RPC_URL" \
    --from "$EXTRACT_FROM" \
    --to "$EXTRACT_TO" \
    "${TXGEN_EXTRACT_ARGS[@]}" \
    -o "$ALL_BLOCKS"
fi

# Split into warmup and measured files
if [ "$WARMUP" -gt 0 ] 2>/dev/null; then
  head -n "$WARMUP" "$ALL_BLOCKS" > "$WARMUP_FILE"
else
  : > "$WARMUP_FILE"
fi
awk -v warmup="$WARMUP" 'NR > warmup { print }' "$ALL_BLOCKS" > "$BENCHMARK_FILE"

if [ "$INCLUDE_BAL" = "true" ] && [ "$BAL_MODE" != "true" ]; then
  echo "Writing no-BAL payload variants for selective BAL mode (${BAL_MODE})"
  for file in "$ALL_BLOCKS" "$WARMUP_FILE" "$BENCHMARK_FILE"; do
    jq -c 'del(.bal, .merged_block_access_list)' "$file" > "${file%.ndjson}-no-bal.ndjson"
  done
fi

echo "Extraction complete: $(wc -l < "$ALL_BLOCKS") payloads in ${OUTPUT_DIR}"
