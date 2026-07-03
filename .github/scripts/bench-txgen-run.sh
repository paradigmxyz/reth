#!/usr/bin/env bash
#
# Runs a single txgen-backed Engine API benchmark cycle:
# mount snapshot → start node → extract source blocks → warmup → send-blocks →
# convert txgen JSON report into legacy benchmark CSVs.
#
# Usage: bench-txgen-run.sh <label> <binary> <output-dir>
#
# Required env: SCHELK_MOUNT, BENCH_RPC_URL, BENCH_BLOCKS, BENCH_WARMUP_BLOCKS
# Optional env: BENCH_BIG_BLOCKS, BENCH_BIG_BLOCKS_TARGET_GAS, BENCH_REORG, BENCH_BAL,
#               BENCH_WORK_DIR, BENCH_WAIT_TIME, BENCH_BASELINE_ARGS,
#               BENCH_FEATURE_ARGS, BENCH_OTLP_TRACES_ENDPOINT,
#               BENCH_OTLP_LOGS_ENDPOINT, BENCH_OTLP_DISABLED,
#               BENCH_TRACING_CHROME, BENCH_TRACY,
#               BENCH_TRACY_FILTER, BENCH_TRACY_SAMPLING_HZ,
#               BENCH_POST_WARMUP_SLEEP_SECONDS,
#               TXGEN_PAYLOADS_DIR (pre-extracted payloads; skips extraction),
#               BENCH_TARGET_METRICS_SCRAPE_INTERVAL_MS (optional txgen override)
set -euxo pipefail

LABEL="$1"
BINARY="$2"
OUTPUT_DIR="$3"

# Resolve git SHA for this run label
GIT_SHA=""
case "$LABEL" in
  baseline*) GIT_SHA="${BASELINE_REF:-}" ;;
  feature*)  GIT_SHA="${FEATURE_REF:-}" ;;
esac

# Resolve git-ref: tag if tagged, otherwise branch name, otherwise raw SHA
GIT_REF="$GIT_SHA"
if [ -n "$GIT_SHA" ]; then
  TAG_NAME=$(git tag --points-at "$GIT_SHA" 2>/dev/null | head -1)
  if [ -n "$TAG_NAME" ]; then
    GIT_REF="$TAG_NAME"
  else
    BRANCH_NAME=$(git branch -r --points-at "$GIT_SHA" 2>/dev/null | sed 's|^ *origin/||' | head -1)
    if [ -n "$BRANCH_NAME" ]; then
      GIT_REF="$BRANCH_NAME"
    fi
  fi
fi

DATADIR_NAME="datadir"
BIG_BLOCKS="${BENCH_BIG_BLOCKS:-false}"
if [ "$BIG_BLOCKS" = "true" ]; then
  DATADIR_NAME="datadir-big-blocks"
fi
DATADIR="$SCHELK_MOUNT/$DATADIR_NAME"
mkdir -p "$OUTPUT_DIR"
LOG="${OUTPUT_DIR}/node.log"
TARGET_METRICS_RANGE="$OUTPUT_DIR/target-metrics-range.json"

RETH_SCOPE="${RETH_SCOPE:-reth-bench.scope}"
BENCH_TARGET_METRICS_SCRAPE_INTERVAL_MS="${BENCH_TARGET_METRICS_SCRAPE_INTERVAL_MS:-}"

capture_unix_time_ms() {
  python3 -c 'import time; print(time.time_ns() // 1_000_000)'
}

record_target_metric_range() {
  local start_ms="$1"
  local end_ms="$2"
  if [ -z "${BENCH_TARGET_METRICS_CONFIG:-}" ]; then
    return 0
  fi

  python3 - "$TARGET_METRICS_RANGE" "$start_ms" "$end_ms" "${BENCH_ID:-}" "$(basename "$OUTPUT_DIR")" <<'PY'
import json
import sys

output_path, start_ms, end_ms, benchmark_id, benchmark_run = sys.argv[1:6]
start_ms = int(start_ms)
end_ms = int(end_ms)

with open(output_path, "w") as f:
    json.dump(
        {
            "benchmark_id": benchmark_id,
            "benchmark_run": benchmark_run,
            "range_start_ms": start_ms,
            "range_end_ms": end_ms,
            "duration_ms": end_ms - start_ms,
        },
        f,
        indent=2,
    )
    f.write("\n")
PY
}

extract_target_metric_scrapes() {
  local samples_path="$1"
  local output_path="$2"
  local filter_output
  local -a filter_lines
  local sample_grep
  local sample_names_json
  local prefiltered_path
  local -a pipe_status

  : > "$output_path"

  if [ ! -f "$samples_path" ]; then
    echo "::error::Target metrics enabled but samples archive is missing: $samples_path"
    return 1
  fi

  filter_output="$(python3 .github/scripts/bench-target-metric-sample-filter.py "$BENCH_TARGET_METRICS_CONFIG")"
  mapfile -t filter_lines <<< "$filter_output"
  sample_grep="${filter_lines[0]}"
  sample_names_json="${filter_lines[1]}"
  prefiltered_path="${output_path}.prefiltered"

  set +e
  gzip -dc "$samples_path" | grep -E -- "$sample_grep" > "$prefiltered_path"
  pipe_status=("${PIPESTATUS[@]}")
  set -e

  if [ "${pipe_status[0]}" -ne 0 ]; then
    echo "::error::Failed to decompress samples archive: $samples_path"
    rm -f "$prefiltered_path"
    return "${pipe_status[0]}"
  fi
  if [ "${pipe_status[1]}" -ne 0 ] && [ "${pipe_status[1]}" -ne 1 ]; then
    echo "::error::Failed to prefilter target metric samples from: $samples_path"
    rm -f "$prefiltered_path"
    return "${pipe_status[1]}"
  fi

  jq -c --argjson metric_names "$sample_names_json" \
    '. as $sample | select(($metric_names | index($sample.name)) != null)' \
    "$prefiltered_path" > "$output_path"
  rm -f "$prefiltered_path"

  echo "Filtered $(wc -l < "$output_path" | tr -d ' ') target metric samples from $samples_path"
}

bal_enabled_for_label() {
  case "${BENCH_BAL:-false}" in
    false|"")
      echo false
      ;;
    true)
      echo true
      ;;
    feature)
      if [[ "$LABEL" == feature* ]]; then echo true; else echo false; fi
      ;;
    baseline)
      if [[ "$LABEL" == baseline* ]]; then echo true; else echo false; fi
      ;;
    *)
      echo "::error::Unknown BENCH_BAL value: ${BENCH_BAL}" >&2
      return 1
      ;;
  esac
}

USE_BAL="$(bal_enabled_for_label)"
echo "BAL replay for ${LABEL}: ${USE_BAL} (mode=${BENCH_BAL:-false})"

call_reth_jit() {
  local action="$1"
  local response
  if ! response=$(curl -sf http://127.0.0.1:8545 -X POST \
    -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"reth_jit\",\"params\":[\"${action}\"],\"id\":1}"); then
    echo "::error::reth_jit ${action} request failed"
    exit 1
  fi
  if jq -e '.error? != null' <<< "$response" > /dev/null 2>&1; then
    echo "::error::reth_jit ${action} failed: ${response}"
    exit 1
  fi
}

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
      sudo pkill -INT -x reth-bb 2>/dev/null || true
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

ONLINE=$(getconf _NPROCESSORS_ONLN)
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
  # txgen reorg mode builds synthetic side-fork blocks via testing_buildBlockV1.
  --http.api eth,net,web3,debug,reth,testing
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
    RETH_ARGS+=(--tracing-otlp="${BENCH_OTLP_TRACES_ENDPOINT}" --tracing-otlp.service-name=reth-bench --tracing-otlp.service-version="${LABEL}")
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

JIT_ENABLED=false
for arg in "${RETH_ARGS[@]}"; do
  if [ "$arg" = "--jit" ]; then
    JIT_ENABLED=true
    break
  fi
done

if [ -n "${BENCH_POST_WARMUP_SLEEP_SECONDS:-}" ]; then
  POST_WARMUP_SLEEP_SECONDS="$BENCH_POST_WARMUP_SLEEP_SECONDS"
elif [ "$JIT_ENABLED" = "true" ]; then
  POST_WARMUP_SLEEP_SECONDS=120
else
  POST_WARMUP_SLEEP_SECONDS=0
fi

SUDO_ENV=()
if [ -n "${OTEL_RESOURCE_ATTRIBUTES:-}" ]; then
  SUDO_ENV+=("OTEL_RESOURCE_ATTRIBUTES=${OTEL_RESOURCE_ATTRIBUTES}")
  SUDO_ENV+=("OTEL_BSP_MAX_QUEUE_SIZE=65536" "OTEL_BLRP_MAX_QUEUE_SIZE=65536")
fi

TOTAL_MEM_KB=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
MEM_LIMIT=$(( TOTAL_MEM_KB * 95 * 1024 / 100 ))
echo "Memory limit: $(( MEM_LIMIT / 1024 / 1024 ))MB (95% of $(( TOTAL_MEM_KB / 1024 ))MB)"

if [ "${BENCH_SAMPLY:-false}" = "true" ]; then
  RETH_ARGS+=(--log.samply)
fi

if [ "${BENCH_TRACING_CHROME:-false}" = "true" ]; then
  if "$BINARY" node --log.tracing-chrome --log.tracing-chrome.file "$OUTPUT_DIR/tracing-chrome-profile.json" --help >/dev/null 2>&1; then
    RETH_ARGS+=(
      --log.tracing-chrome
      --log.tracing-chrome.file "$OUTPUT_DIR/tracing-chrome-profile.json"
    )
  else
    echo "Chrome trace recording requested, but ${LABEL} binary rejected --log.tracing-chrome; skipping"
  fi
fi

if [ "${BENCH_SAMPLY:-false}" = "true" ]; then
  SAMPLY="$(which samply)"
  # shellcheck disable=SC2024
  sudo systemd-run --quiet --scope --collect --unit="$RETH_SCOPE" \
    -p MemoryMax="$MEM_LIMIT" -p AllowedCPUs="$RETH_CPUS" \
    env "${SUDO_ENV[@]}" nice -n -20 \
    "$SAMPLY" record --save-only --presymbolicate --rate 10000 \
    --output "$OUTPUT_DIR/samply-profile.json.gz" \
    -- "$BINARY" "${RETH_ARGS[@]}" \
    > "$LOG" 2>&1 &
else
  # shellcheck disable=SC2024
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

TXGEN_BENCH="$(which bench)"
BENCH_NICE="sudo nice -n -20 sudo -u $(id -un)"
TXGEN_SEND_ARGS=()
if [ -n "${BENCH_WAIT_TIME:-}" ]; then
  TXGEN_SEND_ARGS+=(--wait-time "$BENCH_WAIT_TIME")
fi
if [ -n "${BENCH_REORG:-}" ]; then
  TXGEN_SEND_ARGS+=(--reorg "$BENCH_REORG")
fi

WARMUP="${BENCH_WARMUP_BLOCKS:-0}"
BLOCKS="${BENCH_BLOCKS:?BENCH_BLOCKS must be set}"
TOTAL=$(( WARMUP + BLOCKS ))
if [ "$BLOCKS" -le 0 ] || [ "$TOTAL" -le 0 ]; then
  echo "::error::BENCH_BLOCKS must be greater than 0"
  exit 1
fi

TXGEN_DIR="$OUTPUT_DIR/txgen"
mkdir -p "$TXGEN_DIR"

# Use pre-extracted payloads if available, otherwise extract inline.
if [ -n "${TXGEN_PAYLOADS_DIR:-}" ] && [ -d "$TXGEN_PAYLOADS_DIR" ]; then
  echo "Using pre-extracted payloads from ${TXGEN_PAYLOADS_DIR}"
  if [ "$BIG_BLOCKS" = "true" ]; then
    WARMUP_BLOCKS="$TXGEN_PAYLOADS_DIR/warmup-big-blocks.ndjson"
    BENCHMARK_BLOCKS="$TXGEN_PAYLOADS_DIR/measured-big-blocks.ndjson"
  else
    WARMUP_BLOCKS="$TXGEN_PAYLOADS_DIR/warmup-blocks.ndjson"
    BENCHMARK_BLOCKS="$TXGEN_PAYLOADS_DIR/benchmark-blocks.ndjson"
  fi
  if [ "$USE_BAL" != "true" ]; then
    WARMUP_NO_BAL="${WARMUP_BLOCKS%.ndjson}-no-bal.ndjson"
    BENCHMARK_NO_BAL="${BENCHMARK_BLOCKS%.ndjson}-no-bal.ndjson"
    if [ -f "$BENCHMARK_NO_BAL" ]; then
      WARMUP_BLOCKS="$WARMUP_NO_BAL"
      BENCHMARK_BLOCKS="$BENCHMARK_NO_BAL"
    fi
  fi
  echo "Selected txgen payloads: warmup=${WARMUP_BLOCKS}, benchmark=${BENCHMARK_BLOCKS}"
  if [ ! -f "$BENCHMARK_BLOCKS" ]; then
    echo "::error::Pre-extracted payloads missing: ${BENCHMARK_BLOCKS}"
    exit 1
  fi
else
  TXGEN_ETHEREUM="$(which txgen-ethereum)"
  HEAD_JSON=$(curl -sf http://127.0.0.1:8545 -X POST \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}')
  HEAD_HEX=$(jq -r '.result' <<< "$HEAD_JSON")
  HEAD_DEC=$((16#${HEAD_HEX#0x}))

  ALL_BLOCKS="$TXGEN_DIR/all-blocks.ndjson"
  WARMUP_BLOCKS="$TXGEN_DIR/warmup-blocks.ndjson"
  BENCHMARK_BLOCKS="$TXGEN_DIR/benchmark-blocks.ndjson"
  if [ "$BIG_BLOCKS" = "true" ]; then
    ALL_BLOCKS="$TXGEN_DIR/all-big-blocks.ndjson"
    WARMUP_BLOCKS="$TXGEN_DIR/warmup-big-blocks.ndjson"
    BENCHMARK_BLOCKS="$TXGEN_DIR/measured-big-blocks.ndjson"
  fi

  EXTRACT_FROM=$(( HEAD_DEC + 1 ))
  TXGEN_EXTRACT_ARGS=()
  if [ "$USE_BAL" = "true" ]; then
    TXGEN_EXTRACT_ARGS+=(--bal)
  fi
  if [ "$BIG_BLOCKS" = "true" ]; then
    echo "Extracting ${TOTAL} big blocks from ${EXTRACT_FROM} for txgen benchmark (${WARMUP} warmup, ${BLOCKS} measured, bal=${USE_BAL})"
    "$TXGEN_ETHEREUM" extract-big-blocks \
      --rpc "$BENCH_RPC_URL" \
      --from "$EXTRACT_FROM" \
      --count "$TOTAL" \
      --target-gas "${BENCH_BIG_BLOCKS_TARGET_GAS:-1G}" \
      "${TXGEN_EXTRACT_ARGS[@]}" \
      -o "$ALL_BLOCKS"
  else
    EXTRACT_TO=$(( HEAD_DEC + TOTAL ))
    echo "Extracting blocks ${EXTRACT_FROM}..${EXTRACT_TO} for txgen benchmark (${WARMUP} warmup, ${BLOCKS} measured, bal=${USE_BAL})"
    "$TXGEN_ETHEREUM" extract \
      --rpc "$BENCH_RPC_URL" \
      --from "$EXTRACT_FROM" \
      --to "$EXTRACT_TO" \
      "${TXGEN_EXTRACT_ARGS[@]}" \
      -o "$ALL_BLOCKS"
  fi

  if [ "$WARMUP" -gt 0 ] 2>/dev/null; then
    head -n "$WARMUP" "$ALL_BLOCKS" > "$WARMUP_BLOCKS"
  else
    : > "$WARMUP_BLOCKS"
  fi
  awk -v warmup="$WARMUP" 'NR > warmup { print }' "$ALL_BLOCKS" > "$BENCHMARK_BLOCKS"
fi

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

if [ "$WARMUP" -gt 0 ] 2>/dev/null && [ "$POST_WARMUP_SLEEP_SECONDS" -gt 0 ] 2>/dev/null; then
  echo "Sleeping ${POST_WARMUP_SLEEP_SECONDS}s after warmup to let background JIT finish..."
  sleep "$POST_WARMUP_SLEEP_SECONDS"
fi

if [ "${BENCH_TRACY:-off}" != "off" ]; then
  echo "Starting tracy-capture..."
  tracy-capture -f -o "$OUTPUT_DIR/tracy-profile.tracy" &
  TRACY_PID=$!
  sleep 0.5
fi

# TODO(txgen): expose microsecond client-side FCU latency to avoid ms rounding.
TARGET_METRICS_START_MS=""
CLICKHOUSE_REPORT=()
if [ -n "${CLICKHOUSE_URL:-}" ]; then
  CLICKHOUSE_REPORT=(--report "clickhouse:$CLICKHOUSE_URL")
fi

METRICS_ARGS=()
PROMETHEUS_REPORT=()
PROMETHEUS_METADATA=()
METRICS_URL_ADDED=false
if [ -n "${BENCH_TARGET_METRICS_CONFIG:-}" ] || [ -n "${BENCH_VICTORIAMETRICS_URL:-}" ]; then
  if [ -z "${BENCH_METRICS_ADDR:-}" ]; then
    echo "::error::BENCH_METRICS_ADDR is required when benchmark metrics are enabled"
    exit 1
  fi

  METRICS_ARGS+=(--metrics-url "http://${BENCH_METRICS_ADDR}/metrics")
  METRICS_URL_ADDED=true
fi

if [ -n "${BENCH_TARGET_METRICS_CONFIG:-}" ]; then
  TARGET_METRICS_START_MS="$(capture_unix_time_ms)"
  if [ "$METRICS_URL_ADDED" = true ] && [ -n "$BENCH_TARGET_METRICS_SCRAPE_INTERVAL_MS" ]; then
    METRICS_ARGS+=(--scrape-interval-ms "$BENCH_TARGET_METRICS_SCRAPE_INTERVAL_MS")
  fi
fi

if [ -n "${BENCH_VICTORIAMETRICS_URL:-}" ]; then
  if [ "$METRICS_URL_ADDED" != true ] && [ -n "${BENCH_METRICS_ADDR:-}" ]; then
    METRICS_ARGS+=(--metrics-url "http://${BENCH_METRICS_ADDR}/metrics")
  fi
  PROMETHEUS_REPORT+=(--report "victoriametrics:$BENCH_VICTORIAMETRICS_URL")

  if [ -n "${BENCH_LABELS_FILE:-}" ] && [ -f "$BENCH_LABELS_FILE" ]; then
    BENCHMARK_START=$(jq -r '.run_start_epoch // empty' "$BENCH_LABELS_FILE")
    if [ -n "$BENCHMARK_START" ]; then
      METRICS_ARGS+=(--metrics-align "$BENCHMARK_START")
    fi

    for key in benchmark_run run_type benchmark_id run_start_epoch reference_epoch bench_sha; do
      value=$(jq -r --arg key "$key" '.[$key] // empty' "$BENCH_LABELS_FILE")
      if [ -n "$value" ]; then
        PROMETHEUS_METADATA+=(-m "$key=$value")
      fi
    done
  fi
fi

if [ "$JIT_ENABLED" = "true" ]; then
  echo "Pausing JIT helper before measured benchmark..."
  call_reth_jit pause
fi

echo "Running txgen measured benchmark (${BLOCKS} blocks)..."
$BENCH_NICE "$TXGEN_BENCH" send-blocks \
  --engine http://127.0.0.1:8551 \
  --jwt-secret "$DATADIR/jwt.hex" \
  --input "$BENCHMARK_BLOCKS" \
  "${TXGEN_SEND_ARGS[@]}" \
  "${METRICS_ARGS[@]}" \
  --wait-for-persistence never \
  --report json:"$OUTPUT_DIR/report.json" \
  "${CLICKHOUSE_REPORT[@]}" \
  "${PROMETHEUS_REPORT[@]}" \
  -m "git-sha=$GIT_SHA" \
  -m "git-ref=$GIT_REF" \
  -m "job=github-reth-bench" \
  -m "platform=ethereum" \
  -m "scenario=replay" \
  -m "bal-mode=${BENCH_BAL:-false}" \
  -m "bal-enabled=$USE_BAL" \
  "${PROMETHEUS_METADATA[@]}" 2>&1 | sed -u "s/^/[bench] /"

if [ -n "$TARGET_METRICS_START_MS" ]; then
  TARGET_METRICS_END_MS="$(capture_unix_time_ms)"
  record_target_metric_range "$TARGET_METRICS_START_MS" "$TARGET_METRICS_END_MS"
  rm -f "$OUTPUT_DIR/target-metrics-scrapes.jsonl"
  extract_target_metric_scrapes \
    "$OUTPUT_DIR/report.samples.ndjson.gz" \
    "$OUTPUT_DIR/target-metrics-scrapes.jsonl"
fi

python3 .github/scripts/bench-txgen-report-to-reth-csv.py "$OUTPUT_DIR/report.json" "$OUTPUT_DIR"
