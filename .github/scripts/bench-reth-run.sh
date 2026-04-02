#!/usr/bin/env bash
#
# Runs a single reth-bench cycle: mount snapshot → start node → warmup →
# benchmark → stop node → recover snapshot.
#
# Usage: bench-reth-run.sh <label> <binary> <output-dir>
#
# Required env: SCHELK_MOUNT, BENCH_RPC_URL, BENCH_BLOCKS, BENCH_WARMUP_BLOCKS
# Optional env: BENCH_BIG_BLOCKS (true/false), BENCH_WORK_DIR (for big blocks path)
#               BENCH_WAIT_TIME (duration like 500ms, default empty)
#               BENCH_BASELINE_ARGS (extra reth node args for baseline runs)
#               BENCH_FEATURE_ARGS (extra reth node args for feature runs)
#               BENCH_OTLP_TRACES_ENDPOINT (OTLP HTTP endpoint for traces, e.g. https://host/insert/opentelemetry/v1/traces)
#               BENCH_OTLP_LOGS_ENDPOINT (OTLP HTTP endpoint for logs, e.g. https://host/insert/opentelemetry/v1/logs)
#               BENCH_OTLP_DISABLED (true to skip OTLP export even if endpoints are set)
set -euo pipefail

LABEL="$1"
BINARY="$2"
OUTPUT_DIR="$3"
DATADIR_NAME="datadir"
if [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
  DATADIR_NAME="datadir-big-blocks"
fi
DATADIR="$SCHELK_MOUNT/$DATADIR_NAME"
mkdir -p "$OUTPUT_DIR"
LOG="${OUTPUT_DIR}/node.log"

cleanup() {
  kill "$TAIL_PID" 2>/dev/null || true
  # Stop tracy-capture first (SIGINT makes it disconnect and flush to disk)
  # Must happen before killing reth, otherwise reth keeps streaming data.
  if [ -n "${TRACY_PID:-}" ] && kill -0 "$TRACY_PID" 2>/dev/null; then
    echo "Stopping tracy-capture..."
    kill -INT "$TRACY_PID" 2>/dev/null || true
    for i in $(seq 1 30); do
      kill -0 "$TRACY_PID" 2>/dev/null || break
      if [ $((i % 10)) -eq 0 ]; then
        echo "Waiting for tracy-capture to finish writing... (${i}s)"
      fi
      sleep 1
    done
    if kill -0 "$TRACY_PID" 2>/dev/null; then
      echo "tracy-capture still running after 30s, killing..."
      kill -9 "$TRACY_PID" 2>/dev/null || true
    fi
    wait "$TRACY_PID" 2>/dev/null || true
  fi
  if [ -n "${RETH_PID:-}" ] && sudo kill -0 "$RETH_PID" 2>/dev/null; then
    if [ "${BENCH_SAMPLY:-false}" = "true" ]; then
      # Send SIGINT to the inner reth process by exact name (not -f which
      # would also match samply's cmdline containing "reth"). Samply will
      # capture reth's exit and save the profile.
      sudo pkill -INT -x reth 2>/dev/null || true
      # Wait for samply to finish writing the profile and exit
      for i in $(seq 1 120); do
        sudo pgrep -x samply > /dev/null 2>&1 || break
        if [ $((i % 10)) -eq 0 ]; then
          echo "Waiting for samply to finish writing profile... (${i}s)"
        fi
        sleep 1
      done
      if sudo pgrep -x samply > /dev/null 2>&1; then
        echo "Samply still running after 120s, sending SIGTERM..."
        sudo pkill -x samply 2>/dev/null || true
      fi
    else
      sudo kill "$RETH_PID"
      for i in $(seq 1 30); do
        sudo kill -0 "$RETH_PID" 2>/dev/null || break
        sleep 1
      done
    fi
    sudo kill -9 "$RETH_PID" 2>/dev/null || true
    sleep 1
  fi
  # Fix ownership of reth-created files (reth runs as root)
  sudo chown -R "$(id -un):$(id -gn)" "$OUTPUT_DIR" 2>/dev/null || true
  if mountpoint -q "$SCHELK_MOUNT"; then
    sudo umount -l "$SCHELK_MOUNT" || true
    sudo schelk recover -y || true
  fi
}
TAIL_PID=
TRACY_PID=
trap cleanup EXIT

# Clean up stale schelk state from a previous cancelled run.
# If schelk thinks it's still mounted (e.g. a cancelled run skipped cleanup),
# recover first to reset state.
sudo schelk recover -y -k || true

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
MAX_RETH=$(( ONLINE - 1 ))
if [ "${BENCH_CORES:-0}" -gt 0 ] && [ "$BENCH_CORES" -lt "$MAX_RETH" ]; then
  MAX_RETH=$BENCH_CORES
fi
RETH_CPUS="1-${MAX_RETH}"

BIG_BLOCKS="${BENCH_BIG_BLOCKS:-false}"

RETH_ARGS=(
  node
  --datadir "$DATADIR"
  --log.file.directory "$OUTPUT_DIR/reth-logs"
  --engine.accept-execution-requests-hash
  --http
  --http.port 8545
  --ws
  --ws.api all
  --authrpc.port 8551
  --disable-discovery
  --no-persist-peers
)

# Gate flag on binary support (older baselines may not have it).
# Uses --help which exits immediately via clap without node init.
SYNC_STATE_IDLE=false
if "$BINARY" node --help 2>/dev/null | grep -qF -- '--debug.startup-sync-state-idle'; then
  RETH_ARGS+=(--debug.startup-sync-state-idle)
  SYNC_STATE_IDLE=true
fi

# Append per-label extra node args (baseline or feature)
EXTRA_NODE_ARGS=""
case "$LABEL" in
  baseline*) EXTRA_NODE_ARGS="${BENCH_BASELINE_ARGS:-}" ;;
  feature*)  EXTRA_NODE_ARGS="${BENCH_FEATURE_ARGS:-}" ;;
esac
if [ -n "$EXTRA_NODE_ARGS" ]; then
  # Word-split the string into individual args
  # shellcheck disable=SC2206
  RETH_ARGS+=($EXTRA_NODE_ARGS)
fi

if [ -n "${BENCH_METRICS_ADDR:-}" ]; then
  RETH_ARGS+=(--metrics "$BENCH_METRICS_ADDR")
fi

# OTLP traces and logs export
if [ "${BENCH_OTLP_DISABLED:-false}" != "true" ]; then
  if [ -n "${BENCH_OTLP_TRACES_ENDPOINT:-}" ]; then
    RETH_ARGS+=(--tracing-otlp="${BENCH_OTLP_TRACES_ENDPOINT}" --tracing-otlp.service-name=reth-bench)
  fi
  if [ -n "${BENCH_OTLP_LOGS_ENDPOINT:-}" ]; then
    RETH_ARGS+=(--logs-otlp="${BENCH_OTLP_LOGS_ENDPOINT}" --logs-otlp.filter=debug)
  fi
fi

# Tracy profiling: add --log.tracy flags and set environment
if [ "${BENCH_TRACY:-off}" != "off" ]; then
  RETH_ARGS+=(--log.tracy --log.tracy.filter "${BENCH_TRACY_FILTER:-debug}")
  if [ "${BENCH_TRACY}" = "on" ]; then
    export TRACY_NO_SYS_TRACE=1
  elif [ "${BENCH_TRACY}" = "full" ]; then
    export TRACY_SAMPLING_HZ="${BENCH_TRACY_SAMPLING_HZ:-1}"
  fi
fi

SUDO_ENV=()
if [ -n "${OTEL_RESOURCE_ATTRIBUTES:-}" ]; then
  SUDO_ENV+=("OTEL_RESOURCE_ATTRIBUTES=${OTEL_RESOURCE_ATTRIBUTES}")
  SUDO_ENV+=("OTEL_BSP_MAX_QUEUE_SIZE=65536" "OTEL_BLRP_MAX_QUEUE_SIZE=65536")
fi

# Limit reth memory to 95% of available RAM to prevent OOM kills
TOTAL_MEM_KB=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
MEM_LIMIT=$(( TOTAL_MEM_KB * 95 / 100 * 1024 ))
echo "Memory limit: $(( MEM_LIMIT / 1024 / 1024 ))MB (95% of $(( TOTAL_MEM_KB / 1024 ))MB)"

if [ "${BENCH_SAMPLY:-false}" = "true" ]; then
  RETH_ARGS+=(--log.samply)
  SAMPLY="$(which samply)"
  sudo systemd-run --scope -p MemoryMax="$MEM_LIMIT" -p AllowedCPUs="$RETH_CPUS" \
    env "${SUDO_ENV[@]}" nice -n -20 \
    "$SAMPLY" record --save-only --presymbolicate --rate 10000 \
    --output "$OUTPUT_DIR/samply-profile.json.gz" \
    -- "$BINARY" "${RETH_ARGS[@]}" \
    > "$LOG" 2>&1 &
else
  sudo systemd-run --scope -p MemoryMax="$MEM_LIMIT" -p AllowedCPUs="$RETH_CPUS" \
    env "${SUDO_ENV[@]}" nice -n -20 "$BINARY" "${RETH_ARGS[@]}" \
    > "$LOG" 2>&1 &
fi

RETH_PID=$!
stdbuf -oL tail -f "$LOG" | sed -u "s/^/[reth] /" &
TAIL_PID=$!

for i in $(seq 1 60); do
  if curl -sf http://127.0.0.1:8545 -X POST \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
    > /dev/null 2>&1; then
    echo "reth (${LABEL}) RPC is up after ${i}s"
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo "::error::reth (${LABEL}) failed to start within 60s"
    cat "$LOG"
    exit 1
  fi
  sleep 1
done

# Wait for the pipeline to finish (eth_syncing returns false) so the
# engine is in live mode and can accept newPayload calls.
# Only possible when --debug.startup-sync-state-idle is supported.
if [ "$SYNC_STATE_IDLE" = "true" ]; then
  for i in $(seq 1 300); do
    SYNC_RESULT=$(curl -sf http://127.0.0.1:8545 -X POST \
      -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","method":"eth_syncing","params":[],"id":1}' 2>/dev/null || true)
    if [ -n "$SYNC_RESULT" ] && jq -e '.result == false' <<< "$SYNC_RESULT" > /dev/null 2>&1; then
      echo "reth (${LABEL}) pipeline finished after ${i}s, engine is live"
      break
    fi
    if [ "$i" -eq 300 ]; then
      echo "::error::reth (${LABEL}) pipeline did not finish within 300s"
      cat "$LOG"
      exit 1
    fi
    sleep 1
  done
else
  echo "reth (${LABEL}) binary does not support --debug.startup-sync-state-idle, skipping sync wait"
fi

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

  BB_BENCH_ARGS=(--reth-new-payload)
  if [ -n "${BENCH_WAIT_TIME:-}" ]; then
    BB_BENCH_ARGS+=(--wait-time "$BENCH_WAIT_TIME")
  fi

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
  if [ "${BENCH_TRACY:-off}" != "off" ]; then
    echo "Starting tracy-capture..."
    tracy-capture -f -o "$OUTPUT_DIR/tracy-profile.tracy" &
    TRACY_PID=$!
    sleep 0.5  # give tracy-capture time to connect
  fi

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
  # Warmup
  $BENCH_NICE "$RETH_BENCH" new-payload-fcu \
    --rpc-url "$BENCH_RPC_URL" \
    --engine-rpc-url http://127.0.0.1:8551 \
    --jwt-secret "$DATADIR/jwt.hex" \
    --advance "${BENCH_WARMUP_BLOCKS:-50}" \
    "${EXTRA_BENCH_ARGS[@]}" 2>&1 | sed -u "s/^/[bench] /"

  # Start tracy-capture after warmup so profile only covers the benchmark
  if [ "${BENCH_TRACY:-off}" != "off" ]; then
    echo "Starting tracy-capture..."
    tracy-capture -f -o "$OUTPUT_DIR/tracy-profile.tracy" &
    TRACY_PID=$!
    sleep 0.5  # give tracy-capture time to connect
  fi

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
