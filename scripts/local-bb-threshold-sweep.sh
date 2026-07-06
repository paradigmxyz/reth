#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

BINARY="${BINARY:-$REPO_ROOT/target/profiling/reth-bb}"
BENCH_BIN="${BENCH_BIN:-$(command -v bench)}"
SCHELK_MOUNT="${SCHELK_MOUNT:-/schelk}"
DATADIR="${DATADIR:-$SCHELK_MOUNT/reth}"
BIG_BLOCKS_FILE="${BIG_BLOCKS_FILE:-$SCHELK_MOUNT/bench/txgen-big-blocks-mainnet-24979001-1000-1G.ndjson}"
NORMAL_BLOCKS_FILE="${NORMAL_BLOCKS_FILE:-$SCHELK_MOUNT/bench/txgen-normal-mainnet-24979001-20000.ndjson}"
OUT_ROOT="${OUT_ROOT:-/home/ubuntu/reth-bench-local/persistence-threshold-sweep-$(date -u +%Y%m%dT%H%M%SZ)}"
THRESHOLDS="${THRESHOLDS:-5 10 15 20 25 30 35 40 45 50}"
BIG_BLOCK_COUNT="${BIG_BLOCK_COUNT:-$(wc -l < "$BIG_BLOCKS_FILE" | tr -d ' ')}"
METRICS_ADDR="${METRICS_ADDR:-127.0.0.1:9001}"
SCRAPE_INTERVAL_MS="${SCRAPE_INTERVAL_MS:-200}"
RETH_SCOPE="${RETH_SCOPE:-reth-bench.scope}"
TARGET_METRICS_CONFIG="${TARGET_METRICS_CONFIG:-$REPO_ROOT/.github/config/bench-metrics-targets.json}"

if [ ! -x "$BINARY" ]; then
  echo "missing executable reth-bb binary: $BINARY" >&2
  exit 1
fi
if [ ! -x "$BENCH_BIN" ]; then
  echo "missing executable bench binary: $BENCH_BIN" >&2
  exit 1
fi
if [ ! -f "$BIG_BLOCKS_FILE" ]; then
  echo "missing big-block stream: $BIG_BLOCKS_FILE" >&2
  exit 1
fi
if [ ! -f "$DATADIR/jwt.hex" ]; then
  echo "missing JWT secret: $DATADIR/jwt.hex" >&2
  exit 1
fi

mkdir -p "$OUT_ROOT"

capture_unix_time_ms() {
  python3 -c 'import time; print(time.time_ns() // 1_000_000)'
}

stream_big_blocks() {
  head -n "$BIG_BLOCK_COUNT" "$BIG_BLOCKS_FILE"
}

stop_node() {
  sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
  sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
}

cleanup() {
  stop_node
  sudo schelk recover -y --kill >/dev/null 2>&1 || true
}
trap cleanup EXIT

write_target_metric_range() {
  local run_dir="$1"
  local start_ms="$2"
  local end_ms="$3"
  local threshold="$4"
  python3 - "$run_dir/target-metrics-range.json" "$start_ms" "$end_ms" "$threshold" <<'PY'
import json
import sys

path, start_ms, end_ms, threshold = sys.argv[1:5]
start_ms = int(start_ms)
end_ms = int(end_ms)
with open(path, "w") as f:
    json.dump(
        {
            "benchmark_id": "local-persistence-threshold-sweep",
            "benchmark_run": f"threshold-{int(threshold):02d}",
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
  local run_dir="$1"
  local samples_path="$run_dir/report.samples.ndjson.gz"
  local output_path="$run_dir/target-metrics-scrapes.jsonl"
  local filter_output sample_grep sample_names_json prefiltered_path

  : > "$output_path"
  if [ ! -f "$samples_path" ]; then
    echo "missing metrics samples archive: $samples_path" >&2
    return 1
  fi

  filter_output="$(python3 .github/scripts/bench-target-metric-sample-filter.py "$TARGET_METRICS_CONFIG")"
  sample_grep="$(sed -n '1p' <<< "$filter_output")"
  sample_names_json="$(sed -n '2p' <<< "$filter_output")"
  prefiltered_path="$output_path.prefiltered"

  set +e
  gzip -dc "$samples_path" | grep -E -- "$sample_grep" > "$prefiltered_path"
  local -a pipe_status=("${PIPESTATUS[@]}")
  set -e

  if [ "${pipe_status[0]}" -ne 0 ]; then
    echo "failed to decompress samples archive: $samples_path" >&2
    rm -f "$prefiltered_path"
    return "${pipe_status[0]}"
  fi
  if [ "${pipe_status[1]}" -ne 0 ] && [ "${pipe_status[1]}" -ne 1 ]; then
    echo "failed to prefilter target metric samples from: $samples_path" >&2
    rm -f "$prefiltered_path"
    return "${pipe_status[1]}"
  fi

  jq -c --argjson metric_names "$sample_names_json" \
    '. as $sample | select(($metric_names | index($sample.name)) != null)' \
    "$prefiltered_path" > "$output_path"
  rm -f "$prefiltered_path"
}

wait_for_rpc() {
  for i in $(seq 1 60); do
    if curl -sf http://127.0.0.1:8545 \
      -H 'content-type: application/json' \
      -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
      >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

wait_for_idle_sync() {
  for i in $(seq 1 300); do
    local response
    response="$(curl -sf http://127.0.0.1:8545 \
      -H 'content-type: application/json' \
      -d '{"jsonrpc":"2.0","method":"eth_syncing","params":[],"id":1}' 2>/dev/null || true)"
    if [ -n "$response" ] && jq -e '.result == false' <<< "$response" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

write_run_metadata() {
  local run_dir="$1"
  local threshold="$2"
  local head_hex="$3"
  python3 - "$run_dir/metadata.json" "$threshold" "$head_hex" <<'PY'
import json
import os
import subprocess
import sys

path, threshold, head_hex = sys.argv[1:4]

def output(cmd):
    return subprocess.check_output(cmd, text=True).strip()

metadata = {
    "threshold": int(threshold),
    "head_hex_before_replay": head_hex,
    "big_blocks_file": os.environ["BIG_BLOCKS_FILE"],
    "normal_blocks_file": os.environ["NORMAL_BLOCKS_FILE"],
    "big_block_count": int(os.environ["BIG_BLOCK_COUNT"]),
    "binary": os.environ["BINARY"],
    "bench": os.environ["BENCH_BIN"],
    "git_sha": output(["git", "rev-parse", "HEAD"]),
    "git_branch": output(["git", "branch", "--show-current"]),
}
with open(path, "w") as f:
    json.dump(metadata, f, indent=2)
    f.write("\n")
PY
}

run_threshold() {
  local threshold="$1"
  local run_dir="$OUT_ROOT/threshold-$(printf '%02d' "$threshold")"
  mkdir -p "$run_dir/reth-logs"

  echo "=== threshold ${threshold}: recover schelk ==="
  stop_node
  sudo schelk recover -y --kill || sudo schelk full-recover -y
  sudo schelk mount -y || true

  if [ ! -d "$DATADIR/db" ] || [ ! -d "$DATADIR/static_files" ]; then
    echo "schelk datadir is not mounted at $DATADIR" >&2
    exit 1
  fi

  sync
  sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches' || true

  local log="$run_dir/node.log"
  local online max_reth reth_cpus total_mem_kb mem_limit
  online="$(getconf _NPROCESSORS_ONLN)"
  max_reth=$((online - 1))
  reth_cpus="1-${max_reth}"
  total_mem_kb="$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)"
  mem_limit=$((total_mem_kb * 95 * 1024 / 100))

  echo "=== threshold ${threshold}: start reth-bb ==="
  # shellcheck disable=SC2024
  sudo systemd-run --quiet --scope --collect --unit="$RETH_SCOPE" \
    -p MemoryMax="$mem_limit" -p AllowedCPUs="$reth_cpus" \
    env nice -n -20 "$BINARY" node \
      --datadir "$DATADIR" \
      --log.file.directory "$run_dir/reth-logs" \
      --engine.accept-execution-requests-hash \
      --http \
      --http.api eth,net,web3,debug,reth,testing \
      --http.port 8545 \
      --ws \
      --ws.api all \
      --authrpc.port 8551 \
      --authrpc.jwtsecret "$DATADIR/jwt.hex" \
      --disable-discovery \
      --no-persist-peers \
      --debug.startup-sync-state-idle \
      --metrics "$METRICS_ADDR" \
      --engine.persistence-threshold "$threshold" \
      > "$log" 2>&1 &

  if ! wait_for_rpc; then
    echo "reth-bb failed to expose RPC for threshold $threshold" >&2
    cat "$log" >&2 || true
    exit 1
  fi
  if ! wait_for_idle_sync; then
    echo "reth-bb did not reach idle sync state for threshold $threshold" >&2
    cat "$log" >&2 || true
    exit 1
  fi

  local head_json head_hex
  head_json="$(curl -sf http://127.0.0.1:8545 \
    -H 'content-type: application/json' \
    -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}')"
  head_hex="$(jq -r '.result' <<< "$head_json")"
  write_run_metadata "$run_dir" "$threshold" "$head_hex"

  echo "=== threshold ${threshold}: stream ${BIG_BLOCK_COUNT} x 1G big blocks ==="
  local start_ms end_ms
  start_ms="$(capture_unix_time_ms)"
  stream_big_blocks | "$BENCH_BIN" send-blocks \
    --engine http://127.0.0.1:8551 \
    --jwt-secret "$DATADIR/jwt.hex" \
    --metrics-url "http://${METRICS_ADDR}/metrics" \
    --scrape-interval-ms "$SCRAPE_INTERVAL_MS" \
    --wait-for-persistence never \
    --report "json:$run_dir/report.json" \
    -m "job=local-reth-bb-threshold-sweep" \
    -m "platform=ethereum" \
    -m "scenario=big-block-replay" \
    -m "persistence-threshold=$threshold" \
    2>&1 | tee "$run_dir/bench.log"
  end_ms="$(capture_unix_time_ms)"

  write_target_metric_range "$run_dir" "$start_ms" "$end_ms" "$threshold"
  extract_target_metric_scrapes "$run_dir"
  python3 .github/scripts/bench-txgen-report-to-reth-csv.py "$run_dir/report.json" "$run_dir"

  stop_node
}

cat > "$OUT_ROOT/run-config.json" <<JSON
{
  "binary": "$BINARY",
  "bench": "$BENCH_BIN",
  "schelk_mount": "$SCHELK_MOUNT",
  "datadir": "$DATADIR",
  "big_blocks_file": "$BIG_BLOCKS_FILE",
  "normal_blocks_file": "$NORMAL_BLOCKS_FILE",
  "big_block_count": $BIG_BLOCK_COUNT,
  "thresholds": "$(tr '\n' ' ' <<< "$THRESHOLDS" | sed 's/ *$//')",
  "metrics_addr": "$METRICS_ADDR",
  "scrape_interval_ms": $SCRAPE_INTERVAL_MS
}
JSON

for threshold in $THRESHOLDS; do
  run_threshold "$threshold"
done

python3 scripts/analyze-local-bb-threshold-sweep.py "$OUT_ROOT" | tee "$OUT_ROOT/analysis.md"
echo "artifacts: $OUT_ROOT"
