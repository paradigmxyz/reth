#!/usr/bin/env bash
#
# Runs benchmarkoor-replay against a schelk-managed Reth datadir.
#
# Usage:
#   bench-benchmarkoor-run.sh prepare <binary> <output-dir>
#   bench-benchmarkoor-run.sh run <label> <binary> <output-dir> <tests-jsonl>
#   bench-benchmarkoor-run.sh restart-node <binary> <label> <output-dir> <phase> <log>
#
# The prepare mode starts Reth on the imported snapshot, replays benchmarkoor's
# gas-bump/funding prerun, and promotes that state as the schelk baseline.
#
# The run mode recovers to that post-prerun baseline for each test, starts
# Reth, then delegates the measured setup/testing split to benchmarkoor-replay's
# native `run-many --mode setup-then-testing --json` mode. The restart-node mode
# is invoked by benchmarkoor-replay between setup and measured testing.
set -euo pipefail

MODE="${1:?mode is required}"
shift

: "${SCHELK_MOUNT:?SCHELK_MOUNT must be set}"
: "${BENCHMARKOOR_BIN:?BENCHMARKOOR_BIN must be set}"
: "${BENCHMARKOOR_SUITE:?BENCHMARKOOR_SUITE must be set}"
: "${BENCHMARKOOR_CONTEXT:?BENCHMARKOOR_CONTEXT must be set}"
: "${BENCHMARKOOR_FORK:?BENCHMARKOOR_FORK must be set}"
: "${BENCHMARKOOR_TEST_TYPE:?BENCHMARKOOR_TEST_TYPE must be set}"
: "${BENCHMARKOOR_METADATA_ROOT:?BENCHMARKOOR_METADATA_ROOT must be set}"
: "${BENCHMARKOOR_CACHE:?BENCHMARKOOR_CACHE must be set}"

SCRIPT_PATH="$(readlink -f "$0")"
DATADIR="${SCHELK_MOUNT}/datadir"
RETH_SCOPE="${RETH_SCOPE:-reth-benchmarkoor.scope}"
ENGINE_URL="${BENCHMARKOOR_ENGINE_URL:-http://127.0.0.1:8551}"
HTTP_URL="http://127.0.0.1:8545"

TAIL_PID=""
CURRENT_LOG=""

cleanup() {
  if [ -n "${TAIL_PID:-}" ]; then
    kill "$TAIL_PID" 2>/dev/null || true
  fi
  sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
  sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
}

if [ "$MODE" != "restart-node" ]; then
  trap cleanup EXIT
fi

safe_name() {
  printf '%s' "$1" | sed -E 's/[^A-Za-z0-9_.-]+/-/g; s/^-+//; s/-+$//' | cut -c1-180
}

drop_caches() {
  sync
  sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
  free -h
  grep Cached /proc/meminfo
}

rpc_call() {
  local method="$1"
  local params="${2:-[]}"
  curl -sf "$HTTP_URL" -X POST \
    -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"${method}\",\"params\":${params},\"id\":1}"
}

prerun_head_file() {
  if [ -n "${BENCH_PRERUN_HEAD_FILE:-}" ]; then
    printf '%s\n' "$BENCH_PRERUN_HEAD_FILE"
  elif [ -n "${BENCH_WORK_DIR:-}" ]; then
    printf '%s\n' "${BENCH_WORK_DIR}/prerun-head.json"
  else
    printf '%s\n' "${SCHELK_MOUNT}/prerun-head.json"
  fi
}

write_prerun_head_marker() {
  local marker head_hex head_dec block_hash
  marker="$(prerun_head_file)"
  head_hex="$(rpc_call eth_blockNumber | jq -er '.result')"
  head_dec=$(( 16#${head_hex#0x} ))
  block_hash="$(rpc_call eth_getBlockByNumber "[\"${head_hex}\",false]" | jq -er '.result.hash')"
  mkdir -p "$(dirname "$marker")"
  jq -n \
    --argjson number "$head_dec" \
    --arg number_hex "$head_hex" \
    --arg hash "$block_hash" \
    '{number: $number, number_hex: $number_hex, hash: $hash}' > "$marker"
  echo "Recorded post-prerun head ${head_dec} (${block_hash}) in ${marker}"
}

wait_for_prerun_head() {
  local marker
  marker="$(prerun_head_file)"
  [ -f "$marker" ] || return 0

  local expected_number_hex expected_hash actual_hash
  expected_number_hex="$(jq -er '.number_hex' "$marker")"
  expected_hash="$(jq -er '.hash' "$marker")"

  for i in $(seq 1 60); do
    actual_hash="$(rpc_call eth_getBlockByNumber "[\"${expected_number_hex}\",false]" \
      | jq -r '.result.hash // empty' 2>/dev/null || true)"
    if [ "$actual_hash" = "$expected_hash" ]; then
      echo "Recovered post-prerun head ${expected_number_hex} (${expected_hash}) after ${i}s"
      return 0
    fi
    sleep 1
  done

  echo "::error::Recovered datadir did not expose expected post-prerun head ${expected_number_hex} (${expected_hash})"
  rpc_call eth_blockNumber || true
  exit 1
}

reth_db_capture() {
  local binary="$1"
  shift

  local -a db_args=(db)
  if [ -f "$DATADIR/genesis.json" ]; then
    db_args+=(--chain "$DATADIR/genesis.json")
  fi
  db_args+=(--datadir "$DATADIR" "$@")

  "$binary" "${db_args[@]}" 2>&1
}

has_table_content() {
  local output="$1"
  [ -n "$output" ] && [[ "$output" != *"No content for the given table key"* ]]
}

has_header_content() {
  local output="$1"
  has_table_content "$output" && {
    [[ "$output" == *"BlockHash"* ]] || [[ "$output" == *"parent_hash"* ]]
  }
}

persisted_prerun_head_present() {
  local binary="$1"
  local expected_number="$2"
  local expected_hash="$3"
  local canonical header

  canonical="$(reth_db_capture "$binary" get mdbx CanonicalHeaders "$expected_number" || true)"
  if ! has_table_content "$canonical" || ! grep -qi "${expected_hash#0x}" <<< "$canonical"; then
    return 1
  fi

  header="$(reth_db_capture "$binary" get static-file headers "$expected_number" || true)"
  if has_header_content "$header"; then
    return 0
  fi

  header="$(reth_db_capture "$binary" get mdbx Headers "$expected_number" || true)"
  has_header_content "$header"
}

wait_for_persisted_prerun_head() {
  local binary="$1"
  local required="${2:-true}"
  local marker
  marker="$(prerun_head_file)"
  [ -f "$marker" ] || return 0

  local expected_number expected_hash
  expected_number="$(jq -er '.number' "$marker")"
  expected_hash="$(jq -er '.hash' "$marker")"

  for i in $(seq 1 120); do
    if persisted_prerun_head_present "$binary" "$expected_number" "$expected_hash"; then
      echo "Post-prerun head ${expected_number} (${expected_hash}) is persisted after ${i}s"
      return 0
    fi
    sleep 1
  done

  if [ "$required" != "true" ]; then
    echo "Post-prerun head ${expected_number} (${expected_hash}) was not confirmed in on-disk Reth tables after 120s"
    return 1
  fi

  echo "::error::Post-prerun head ${expected_number} (${expected_hash}) was not found in on-disk Reth tables"
  echo "CanonicalHeaders:"
  reth_db_capture "$binary" get mdbx CanonicalHeaders "$expected_number" || true
  echo "Static-file header:"
  reth_db_capture "$binary" get static-file headers "$expected_number" || true
  echo "MDBX header:"
  reth_db_capture "$binary" get mdbx Headers "$expected_number" || true
  exit 1
}

stop_node() {
  if [ -n "${TAIL_PID:-}" ]; then
    kill "$TAIL_PID" 2>/dev/null || true
    TAIL_PID=""
  fi
  sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
  sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
}

recover_schelk() {
  stop_node
  sudo schelk recover -y --kill || sudo schelk full-recover -y || true
  sudo schelk mount -y
  if [ ! -d "$DATADIR/db" ] || [ ! -d "$DATADIR/static_files" ]; then
    echo "::error::Failed to mount benchmark datadir at ${DATADIR}"
    ls -la "$SCHELK_MOUNT" || true
    ls -la "$DATADIR" || true
    exit 1
  fi
  drop_caches
}

start_node() {
  local binary="$1"
  local label="$2"
  local output_dir="$3"
  local phase="$4"
  local log_path="${5:-}"
  local append_log="${6:-false}"
  local follow_log="${7:-true}"

  mkdir -p "${output_dir}/logs"
  if [ -z "$log_path" ]; then
    log_path="${output_dir}/logs/$(safe_name "${label}-${phase}").log"
  fi
  CURRENT_LOG="$log_path"
  if [ "$append_log" = "true" ]; then
    : >> "$CURRENT_LOG"
  else
    : > "$CURRENT_LOG"
  fi

  stop_node

  local online max_reth reth_cpus
  online="$(nproc --all)"
  max_reth=$(( online - 1 ))
  if [ "${BENCH_CORES:-0}" -gt 0 ] && [ "$BENCH_CORES" -lt "$max_reth" ]; then
    max_reth="$BENCH_CORES"
  fi
  if [ "$max_reth" -lt 1 ]; then
    reth_cpus="0"
  else
    reth_cpus="1-${max_reth}"
  fi

  local -a reth_args=(
    node
    --datadir "$DATADIR"
    --log.file.directory "$output_dir/reth-logs"
    --engine.accept-execution-requests-hash
    --http
    --http.port 8545
    --ws
    --ws.api all
    --authrpc.port 8551
    --disable-discovery
    --no-persist-peers
  )

  if [ -f "$DATADIR/genesis.json" ]; then
    reth_args+=(--chain "$DATADIR/genesis.json")
  fi

  local node_help
  node_help="$("$binary" node --help 2>/dev/null || true)"

  local sync_state_idle=false
  if grep -qF -- '--debug.startup-sync-state-idle' <<< "$node_help"; then
    reth_args+=(--debug.startup-sync-state-idle)
    sync_state_idle=true
  fi

  if [[ "$phase" == "prerun" || "$phase" == *-setup ]] &&
    grep -qF -- '--engine.persistence-threshold' <<< "$node_help" &&
    grep -qF -- '--engine.persistence-backpressure-threshold' <<< "$node_help"; then
    reth_args+=(--engine.persistence-threshold 0 --engine.persistence-backpressure-threshold 1)
  fi

  local extra_node_args=""
  case "$label" in
    baseline*) extra_node_args="${BENCH_BASELINE_ARGS:-}" ;;
    feature*) extra_node_args="${BENCH_FEATURE_ARGS:-}" ;;
  esac
  if [ -n "$extra_node_args" ]; then
    # shellcheck disable=SC2206
    reth_args+=($extra_node_args)
  fi

  if [ -n "${BENCH_METRICS_ADDR:-}" ]; then
    reth_args+=(--metrics "$BENCH_METRICS_ADDR")
  fi

  local mem_limit
  mem_limit="${BENCH_MEMORY_MAX:-32G}"
  if [ -z "$mem_limit" ] || [ "$mem_limit" = "0" ]; then
    local total_mem_kb
    total_mem_kb="$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)"
    mem_limit=$(( total_mem_kb * 95 / 100 * 1024 ))
  fi

  echo "Starting reth (${label}/${phase}) with AllowedCPUs=${reth_cpus}, MemoryMax=${mem_limit}"

  sudo systemd-run --quiet --scope --collect --unit="$RETH_SCOPE" \
    -p MemoryMax="$mem_limit" -p AllowedCPUs="$reth_cpus" \
    nice -n -20 "$binary" "${reth_args[@]}" \
    >> "$CURRENT_LOG" 2>&1 &

  if [ "$follow_log" = "true" ]; then
    stdbuf -oL tail -f "$CURRENT_LOG" | sed -u "s/^/[reth] /" &
    TAIL_PID=$!
  fi

  for i in $(seq 1 60); do
    if curl -sf "$HTTP_URL" -X POST \
      -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
      > /dev/null 2>&1; then
      echo "reth (${label}/${phase}) RPC is up after ${i}s"
      break
    fi
    if [ "$i" -eq 60 ]; then
      echo "::error::reth (${label}/${phase}) failed to start within 60s"
      cat "$CURRENT_LOG"
      exit 1
    fi
    sleep 1
  done

  if [ "$sync_state_idle" = true ]; then
    for i in $(seq 1 300); do
      sync_result="$(curl -sf "$HTTP_URL" -X POST \
        -H 'Content-Type: application/json' \
        -d '{"jsonrpc":"2.0","method":"eth_syncing","params":[],"id":1}' 2>/dev/null || true)"
      if [ -n "$sync_result" ] && jq -e '.result == false' <<< "$sync_result" > /dev/null 2>&1; then
        echo "reth (${label}/${phase}) pipeline finished after ${i}s"
        break
      fi
      if [ "$i" -eq 300 ]; then
        echo "::error::reth (${label}/${phase}) pipeline did not finish within 300s"
        cat "$CURRENT_LOG"
        exit 1
      fi
      sleep 1
    done
  fi

  if [[ "$phase" == *-setup ]]; then
    wait_for_prerun_head
  fi
}

benchmarkoor_common_args() {
  local binary="$1"
  local schelk_bin="$2"
  printf '%s\0' \
    --suite "$BENCHMARKOOR_SUITE" \
    --context "$BENCHMARKOOR_CONTEXT" \
    --fork "$BENCHMARKOOR_FORK" \
    --test-type "$BENCHMARKOOR_TEST_TYPE" \
    --metadata-root "$BENCHMARKOOR_METADATA_ROOT" \
    --cache-dir "$BENCHMARKOOR_CACHE" \
    --engine-url "$ENGINE_URL" \
    --jwt-secret "$DATADIR/jwt.hex" \
    --reth-bin "$binary" \
    --schelk-bin "$schelk_bin"
}

run_benchmarkoor() {
  sudo env \
    "PATH=$PATH" \
    "SCHELK_MOUNT=$SCHELK_MOUNT" \
    "RETH_SCOPE=$RETH_SCOPE" \
    "BENCHMARKOOR_BIN=$BENCHMARKOOR_BIN" \
    "BENCHMARKOOR_SUITE=$BENCHMARKOOR_SUITE" \
    "BENCHMARKOOR_CONTEXT=$BENCHMARKOOR_CONTEXT" \
    "BENCHMARKOOR_FORK=$BENCHMARKOOR_FORK" \
    "BENCHMARKOOR_TEST_TYPE=$BENCHMARKOOR_TEST_TYPE" \
    "BENCHMARKOOR_METADATA_ROOT=$BENCHMARKOOR_METADATA_ROOT" \
    "BENCHMARKOOR_CACHE=$BENCHMARKOOR_CACHE" \
    "BENCH_CORES=${BENCH_CORES:-6}" \
    "BENCH_MEMORY_MAX=${BENCH_MEMORY_MAX:-32G}" \
    "BENCH_BASELINE_ARGS=${BENCH_BASELINE_ARGS:-}" \
    "BENCH_FEATURE_ARGS=${BENCH_FEATURE_ARGS:-}" \
    "BENCH_METRICS_ADDR=${BENCH_METRICS_ADDR:-}" \
    nice -n -20 "$BENCHMARKOOR_BIN" "$@"
}

shell_command() {
  printf '%q ' "$@"
}

if [ "$MODE" = "restart-node" ]; then
  binary="${1:?binary is required}"
  label="${2:?label is required}"
  output_dir="${3:?output directory is required}"
  phase="${4:?phase is required}"
  log_path="${5:?log path is required}"
  start_node "$binary" "$label" "$output_dir" "$phase" "$log_path" true false
  exit 0
fi

if [ "$MODE" = "prepare" ]; then
  binary="${1:?binary is required}"
  output_dir="${2:?output directory is required}"
  mkdir -p "$output_dir"

  recover_schelk
  start_node "$binary" "prepare" "$output_dir" "prerun"

  sudo_schelk="${output_dir}/sudo-schelk"
  cat > "$sudo_schelk" <<'SH'
#!/usr/bin/env bash
exec sudo schelk "$@" -y
SH
  chmod +x "$sudo_schelk"

  mapfile -d '' common < <(benchmarkoor_common_args "$binary" "$sudo_schelk")
  run_benchmarkoor "${common[@]}" baseline prepare --no-mount
  write_prerun_head_marker
  wait_for_persisted_prerun_head "$binary" false ||
    echo "Post-prerun head was not confirmed before shutdown; checking again after graceful stop"

  stop_node
  wait_for_persisted_prerun_head "$binary" true
  sudo schelk promote -y
  sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
  echo "Promoted schelk baseline after benchmarkoor gas-bump/funding"
  exit 0
fi

if [ "$MODE" != "run" ]; then
  echo "Usage: $0 prepare <binary> <output-dir> | run <label> <binary> <output-dir> <tests-jsonl> | restart-node <binary> <label> <output-dir> <phase> <log>"
  exit 1
fi

label="${1:?label is required}"
binary="${2:?binary is required}"
output_dir="${3:?output directory is required}"
tests_jsonl="${4:?tests jsonl is required}"
results="${output_dir}/results.jsonl"
mkdir -p "$output_dir/logs"
: > "$results"

mapfile -d '' common < <(benchmarkoor_common_args "$binary" "schelk")
run_type="feature"
if [[ "$label" == baseline* ]]; then
  run_type="baseline"
fi

while IFS= read -r test_entry; do
  [ -n "$test_entry" ] || continue
  test_name="$(jq -r '.name' <<< "$test_entry")"
  gas_bucket="$(jq -r '.gas_bucket // ""' <<< "$test_entry")"
  test_slug="$(safe_name "$test_name")"
  log_path="${output_dir}/logs/$(safe_name "${label}-${test_slug}").log"
  native_out="${output_dir}/native-$(safe_name "$test_slug").jsonl"
  before_count="$(wc -l < "$results" | tr -d ' ')"

  echo "=== ${label}: ${test_name} ==="
  recover_schelk
  start_node "$binary" "$label" "$output_dir" "${test_slug}-setup" "$log_path" false true

  restart_command="$(
    shell_command "$SCRIPT_PATH" restart-node "$binary" "$label" "$output_dir" "${test_slug}-testing" "$log_path"
  )"

  run_benchmarkoor "${common[@]}" run-many \
    --exact "$test_name" \
    --limit 1 \
    --repetitions 1 \
    --mode setup-then-testing \
    --no-schelk \
    --drop-caches \
    --restart-node-command "$restart_command" \
    --json 2>&1 | tee "$native_out"

  jq -Rnc \
    --arg label "$label" \
    --arg run_type "$run_type" \
    --arg gas_bucket "$gas_bucket" \
    'inputs | fromjson? | select(.kind == "run_many_result") |
      . + {label: $label, run_type: $run_type, gas_bucket: $gas_bucket}' \
    "$native_out" >> "$results"

  after_count="$(wc -l < "$results" | tr -d ' ')"
  if [ "$after_count" -le "$before_count" ]; then
    echo "::error::benchmarkoor-replay did not emit a run_many_result for ${test_name}"
    exit 1
  fi
done < "$tests_jsonl"

sudo schelk recover -y --kill || true
echo "results=${results}"
