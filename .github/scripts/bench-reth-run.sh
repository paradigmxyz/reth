#!/usr/bin/env bash
#
# Runs a single reth-bench cycle: mount snapshot → start node → warmup →
# benchmark → stop node → recover snapshot.
#
# Usage: bench-reth-run.sh <label> <binary> <output-dir>
#
# Required env: SCHELK_MOUNT, BENCH_RPC_URL, BENCH_BLOCKS, BENCH_WARMUP_BLOCKS
set -euo pipefail

LABEL="$1"
BINARY="$2"
OUTPUT_DIR="$3"
DATADIR="$SCHELK_MOUNT/datadir"
LOG="/tmp/reth-bench-node-${LABEL}.log"

cleanup() {
  kill "$TAIL_PID" 2>/dev/null || true
  if [ -n "${RETH_PID:-}" ] && sudo kill -0 "$RETH_PID" 2>/dev/null; then
    sudo kill "$RETH_PID"
    for i in $(seq 1 30); do
      sudo kill -0 "$RETH_PID" 2>/dev/null || break
      sleep 1
    done
    sudo kill -9 "$RETH_PID" 2>/dev/null || true
    sleep 1
  fi
  if ! sudo schelk recover -y 2>/dev/null; then
    # recover can fail with "target is busy" after cancellation, force-cleanup
    sudo umount "$SCHELK_MOUNT" 2>/dev/null || sudo umount -l "$SCHELK_MOUNT" 2>/dev/null || true
    sudo dmsetup remove bench_era 2>/dev/null || true
  fi
}
TAIL_PID=
trap cleanup EXIT

# Mount
sudo schelk mount -y
sync
sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
echo "=== Cache state after drop ==="
free -h
grep Cached /proc/meminfo

# Start reth
# CPU layout: core 0 = OS/IRQs/reth-bench/aux, cores 1+ = reth node
RETH_BENCH="$(which reth-bench)"
ONLINE=$(nproc --all)
RETH_CPUS="1-$(( ONLINE - 1 ))"
sudo taskset -c "$RETH_CPUS" nice -n -20 "$BINARY" node \
  --datadir "$DATADIR" \
  --engine.accept-execution-requests-hash \
  --http \
  --http.port 8545 \
  --ws \
  --ws.api all \
  --authrpc.port 8551 \
  --disable-discovery \
  --no-persist-peers \
  > "$LOG" 2>&1 &

RETH_PID=$!
stdbuf -oL tail -f "$LOG" | sed -u "s/^/[reth] /" &
TAIL_PID=$!

for i in $(seq 1 60); do
  if curl -sf http://127.0.0.1:8545 -X POST \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
    > /dev/null 2>&1; then
    echo "reth (${LABEL}) is ready after ${i}s"
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo "::error::reth (${LABEL}) failed to start within 60s"
    cat "$LOG"
    exit 1
  fi
  sleep 1
done

# Warmup
sudo nice -n -20 "$RETH_BENCH" new-payload-fcu \
  --rpc-url "$BENCH_RPC_URL" \
  --engine-rpc-url http://127.0.0.1:8551 \
  --jwt-secret "$DATADIR/jwt.hex" \
  --advance "${BENCH_WARMUP_BLOCKS:-50}" \
  --reth-new-payload 2>&1 | sed -u "s/^/[bench] /"

# Benchmark
sudo nice -n -20 "$RETH_BENCH" new-payload-fcu \
  --rpc-url "$BENCH_RPC_URL" \
  --engine-rpc-url http://127.0.0.1:8551 \
  --jwt-secret "$DATADIR/jwt.hex" \
  --advance "$BENCH_BLOCKS" \
  --reth-new-payload \
  --output "$OUTPUT_DIR" 2>&1 | sed -u "s/^/[bench] /"

# cleanup runs via trap
