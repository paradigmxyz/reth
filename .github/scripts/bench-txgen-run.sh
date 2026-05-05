#!/usr/bin/env bash
#
# Runs a single txgen-backed Engine API benchmark cycle:
# mount snapshot → start node → extract source blocks → warmup → send-blocks →
# convert txgen JSON report into the legacy reth-bench CSVs.
#
# Usage: bench-txgen-run.sh <label> <binary> <output-dir>
#
# Required env: SCHELK_MOUNT, BENCH_RPC_URL, BENCH_BLOCKS, BENCH_WARMUP_BLOCKS
# Optional env: BENCH_WORK_DIR, BENCH_WAIT_TIME, BENCH_BASELINE_ARGS,
#               BENCH_FEATURE_ARGS, BENCH_OTLP_TRACES_ENDPOINT,
#               BENCH_OTLP_LOGS_ENDPOINT, BENCH_OTLP_DISABLED,
#               BENCH_TRACY, BENCH_TRACY_FILTER, BENCH_TRACY_SAMPLING_HZ
set -euxo pipefail

LABEL="$1"
BINARY="$2"
OUTPUT_DIR="$3"
DATADIR="$SCHELK_MOUNT/datadir"
mkdir -p "$OUTPUT_DIR"
LOG="${OUTPUT_DIR}/node.log"

RETH_SCOPE="${RETH_SCOPE:-reth-bench.scope}"

# Unsupported txgen-path behavior is made explicit here. Keep these checks near
# the top so new txgen support can be added one feature at a time.
if [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
  echo "::error::txgen driver does not support big-block benchmarks yet; use the reth-bench driver"
  exit 1
fi
if [ -n "${BENCH_BAL:-}" ] && [ "${BENCH_BAL}" != "false" ]; then
  echo "::error::txgen driver does not support BAL replay yet; use big-blocks with the reth-bench driver"
  exit 1
fi

cleanup() {
  kill "${TAIL_PID:-}" 2>/dev/null || true
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
  if sudo systemctl is-active "$RETH_SCOPE" >/dev/null 2>&1; then
    if [ "${BENCH_SAMPLY:-false}" = "true" ]; then
      sudo pkill -INT -x reth 2>/dev/null || true
      for i in $(seq 1 120); do
        sudo pgrep -x samply > /dev/null 2>&1 || break
        if [ $((i % 10)) -eq 0 ]; then
          echo "Waiting for samply to finish writing profile... (${i}s)"
        fi
        sleep 1
      done
      sudo pkill -x samply 2>/dev/null || true
    fi
    sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
    sleep 1
  fi
  sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
  sudo chown -R "$(id -un):$(id -gn)" "$OUTPUT_DIR" 2>/dev/null || true
  sudo schelk recover -y --kill || true
}
TAIL_PID=
TRACY_PID=
trap cleanup EXIT

sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
sudo schelk recover -y --kill || sudo schelk full-recover -y || true

sudo schelk mount -y || true
if [ ! -d "$DATADIR/db" ] || [ ! -d "$DATADIR/static_files" ]; then
  echo "::error::Failed to mount benchmark datadir at ${DATADIR}"
  ls -la "$SCHELK_MOUNT" || true
  ls -la "$DATADIR" || true
  exit 1
fi
sync
sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
echo "=== Cache state after drop ==="
free -h
grep Cached /proc/meminfo

ONLINE=$(nproc --all)
MAX_RETH=$(( ONLINE - 1 ))
if [ "${BENCH_CORES:-0}" -gt 0 ] && [ "$BENCH_CORES" -lt "$MAX_RETH" ]; then
  MAX_RETH=$BENCH_CORES
fi
RETH_CPUS="1-${MAX_RETH}"

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

SYNC_STATE_IDLE=false
if "$BINARY" node --help 2>/dev/null | grep -qF -- '--debug.startup-sync-state-idle'; then
  RETH_ARGS+=(--debug.startup-sync-state-idle)
  SYNC_STATE_IDLE=true
fi

EXTRA_NODE_ARGS=""
case "$LABEL" in
  baseline*) EXTRA_NODE_ARGS="${BENCH_BASELINE_ARGS:-}" ;;
  feature*)  EXTRA_NODE_ARGS="${BENCH_FEATURE_ARGS:-}" ;;
esac
if [ -n "$EXTRA_NODE_ARGS" ]; then
  # shellcheck disable=SC2206
  RETH_ARGS+=($EXTRA_NODE_ARGS)
fi

if [ -n "${BENCH_METRICS_ADDR:-}" ]; then
  RETH_ARGS+=(--metrics "$BENCH_METRICS_ADDR")
fi

if [ "${BENCH_OTLP_DISABLED:-false}" != "true" ]; then
  if [ -n "${BENCH_OTLP_TRACES_ENDPOINT:-}" ]; then
    RETH_ARGS+=(--tracing-otlp="${BENCH_OTLP_TRACES_ENDPOINT}" --tracing-otlp.service-name=reth-bench)
  fi
  if [ -n "${BENCH_OTLP_LOGS_ENDPOINT:-}" ]; then
    RETH_ARGS+=(--logs-otlp="${BENCH_OTLP_LOGS_ENDPOINT}" --logs-otlp.filter=debug)
  fi
fi

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

TOTAL_MEM_KB=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
MEM_LIMIT=$(( TOTAL_MEM_KB * 95 / 100 * 1024 ))
echo "Memory limit: $(( MEM_LIMIT / 1024 / 1024 ))MB (95% of $(( TOTAL_MEM_KB / 1024 ))MB)"

if [ "${BENCH_SAMPLY:-false}" = "true" ]; then
  RETH_ARGS+=(--log.samply)
  SAMPLY="$(which samply)"
  sudo systemd-run --quiet --scope --collect --unit="$RETH_SCOPE" \
    -p MemoryMax="$MEM_LIMIT" -p AllowedCPUs="$RETH_CPUS" \
    env "${SUDO_ENV[@]}" nice -n -20 \
    "$SAMPLY" record --save-only --presymbolicate --rate 10000 \
    --output "$OUTPUT_DIR/samply-profile.json.gz" \
    -- "$BINARY" "${RETH_ARGS[@]}" \
    > "$LOG" 2>&1 &
else
  sudo systemd-run --quiet --scope --collect --unit="$RETH_SCOPE" \
    -p MemoryMax="$MEM_LIMIT" -p AllowedCPUs="$RETH_CPUS" \
    env "${SUDO_ENV[@]}" nice -n -20 "$BINARY" "${RETH_ARGS[@]}" \
    > "$LOG" 2>&1 &
fi
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

TXGEN_ETHEREUM="$(which txgen-ethereum)"
TXGEN_BENCH="$(which bench)"
BENCH_NICE="sudo nice -n -20 sudo -u $(id -un)"
TXGEN_SEND_ARGS=()
if [ -n "${BENCH_WAIT_TIME:-}" ]; then
  TXGEN_SEND_ARGS+=(--wait-time "$BENCH_WAIT_TIME")
fi

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
echo "Extracting blocks ${EXTRACT_FROM}..${EXTRACT_TO} for txgen benchmark (${WARMUP} warmup, ${BLOCKS} measured)"
"$TXGEN_ETHEREUM" extract \
  --rpc "$BENCH_RPC_URL" \
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
    "${TXGEN_SEND_ARGS[@]}" \
    --wait-for-persistence never \
    --report json:"$TXGEN_DIR/warmup-report.json" 2>&1 | sed -u "s/^/[bench] /"
else
  echo "Skipping warmup (0 blocks)..."
fi

if [ "${BENCH_TRACY:-off}" != "off" ]; then
  echo "Starting tracy-capture..."
  tracy-capture -f -o "$OUTPUT_DIR/tracy-profile.tracy" &
  TRACY_PID=$!
  sleep 0.5
fi

# TODO(txgen): expose microsecond client-side FCU latency to avoid ms rounding.
# TODO(txgen): support reth-bb payload/env-switch/BAL replay so big-blocks can move here.
echo "Running txgen measured benchmark (${BLOCKS} blocks)..."
$BENCH_NICE "$TXGEN_BENCH" send-blocks \
  --engine http://127.0.0.1:8551 \
  --jwt-secret "$DATADIR/jwt.hex" \
  --input "$BENCHMARK_BLOCKS" \
  "${TXGEN_SEND_ARGS[@]}" \
  --wait-for-persistence never \
  --report json:"$OUTPUT_DIR/report.json" 2>&1 | sed -u "s/^/[bench] /"

python3 .github/scripts/bench-txgen-report-to-reth-csv.py "$OUTPUT_DIR/report.json" "$OUTPUT_DIR"
