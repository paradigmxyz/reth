#!/usr/bin/env bash
#
# Runs benchmarkoor-replay against a Reth datadir mounted by schelk.
#
# Usage:
#   bench-benchmarkoor-run.sh prepare <binary> <output-dir>
#   bench-benchmarkoor-run.sh run <label> <binary> <output-dir> <tests-jsonl>
#   bench-benchmarkoor-run.sh restart-node <binary> <label> <output-dir> <phase> <log>
#
# The prepare mode starts Reth on the imported snapshot and replays benchmarkoor's
# gas-bump/funding prerun. With BENCH_RESET_STRATEGY=schelk, it promotes that
# state as the schelk baseline. With BENCH_RESET_STRATEGY=unwind, it leaves the
# scratch volume mounted at the post-prerun head and later uses Reth unwind to
# reset each test.
#
# The run mode resets to the post-prerun baseline for each test, starts Reth,
# then delegates the measured setup/testing split to benchmarkoor-replay's native
# `run-many --mode setup-then-testing --json` mode. The restart-node mode is
# invoked by benchmarkoor-replay between setup and measured testing.
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
JWT_SECRET="${DATADIR}/jwt.hex"
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

now_ns() {
  date +%s%N
}

elapsed_secs() {
  local start_ns="$1"
  local end_ns
  end_ns="$(now_ns)"
  awk -v ns="$(( end_ns - start_ns ))" 'BEGIN { printf "%.6f", ns / 1000000000 }'
}

reset_strategy() {
  case "${BENCH_RESET_STRATEGY:-schelk}" in
    unwind | schelk) printf '%s\n' "${BENCH_RESET_STRATEGY:-schelk}" ;;
    *)
      echo "::error::BENCH_RESET_STRATEGY must be 'unwind' or 'schelk', got '${BENCH_RESET_STRATEGY}'"
      exit 1
      ;;
  esac
}

ensure_jwt_secret() {
  sudo mkdir -p "$DATADIR"
  if ! sudo test -s "$JWT_SECRET"; then
    local secret
    secret="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
    printf '%s\n' "$secret" | sudo tee "$JWT_SECRET" >/dev/null
    sudo chmod 0600 "$JWT_SECRET"
  fi
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

reth_db_capture() {
  local binary="$1"
  shift

  local -a db_args=(db)
  if [ -f "$DATADIR/genesis.json" ]; then
    db_args+=(--chain "$DATADIR/genesis.json")
  fi
  db_args+=(--datadir "$DATADIR" "$@")

  sudo "$binary" "${db_args[@]}" 2>&1
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

parse_stage_checkpoint_block() {
  sed -nE 's/.*block_number: ([0-9]+).*/\1/p' | head -n 1
}

output_has_expected_hash() {
  local output="$1"
  local expected_hash="$2"
  has_table_content "$output" && grep -qi "${expected_hash#0x}" <<< "$output"
}

finish_stage_matches() {
  local binary="$1"
  local expected_number="$2"
  local finish finish_number
  finish="$(reth_db_capture "$binary" stage-checkpoints get --stage finish || true)"
  finish_number="$(parse_stage_checkpoint_block <<< "$finish")"
  [ -n "$finish_number" ] && [ "$finish_number" -eq "$expected_number" ]
}

persisted_prerun_head_present() {
  local binary="$1"
  local expected_number="$2"
  local expected_hash="$3"
  local canonical header

  header="$(reth_db_capture "$binary" get static-file headers "$expected_number" || true)"
  if has_header_content "$header" && output_has_expected_hash "$header" "$expected_hash"; then
    finish_stage_matches "$binary" "$expected_number"
    return $?
  fi

  canonical="$(reth_db_capture "$binary" get mdbx CanonicalHeaders "$expected_number" || true)"
  if output_has_expected_hash "$canonical" "$expected_hash"; then
    header="$(reth_db_capture "$binary" get mdbx Headers "$expected_number" || true)"
    if has_header_content "$header"; then
      finish_stage_matches "$binary" "$expected_number"
      return $?
    fi
  fi

  return 1
}

print_prerun_persistence_diagnostics() {
  local binary="$1"
  local expected_number="$2"
  echo "Static-file header:"
  reth_db_capture "$binary" get static-file headers "$expected_number" || true
  echo "Finish stage checkpoint:"
  reth_db_capture "$binary" stage-checkpoints get --stage finish || true
  echo "CanonicalHeaders:"
  reth_db_capture "$binary" get mdbx CanonicalHeaders "$expected_number" || true
  echo "MDBX header:"
  reth_db_capture "$binary" get mdbx Headers "$expected_number" || true
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
      echo "Post-prerun head ${expected_number} (${expected_hash}) is persisted and committed after ${i}s"
      return 0
    fi
    sleep 1
  done

  if [ "$required" != "true" ]; then
    echo "Post-prerun head ${expected_number} (${expected_hash}) was not confirmed by static files and Finish checkpoint after 120s"
    return 1
  fi

  echo "::error::Post-prerun head ${expected_number} (${expected_hash}) was not confirmed by static files and Finish checkpoint"
  print_prerun_persistence_diagnostics "$binary" "$expected_number"
  exit 1
}

reth_stage_unwind() {
  local binary="$1"
  local target="$2"

  local -a unwind_args=(stage unwind)
  if [ -f "$DATADIR/genesis.json" ]; then
    unwind_args+=(--chain "$DATADIR/genesis.json")
  fi
  unwind_args+=(--datadir "$DATADIR" to-block "$target")

  sudo "$binary" "${unwind_args[@]}"
}

unwind_to_prerun_head() {
  local binary="$1"
  local marker
  marker="$(prerun_head_file)"
  if [ ! -f "$marker" ]; then
    echo "::error::Missing post-prerun head marker at ${marker}; cannot use unwind reset strategy"
    exit 1
  fi

  local expected_number expected_hash start elapsed finish finish_number
  expected_number="$(jq -er '.number' "$marker")"
  expected_hash="$(jq -er '.hash' "$marker")"

  start="$(now_ns)"
  echo "Unwinding Reth datadir to post-prerun head ${expected_number} (${expected_hash})"
  reth_stage_unwind "$binary" "$expected_number"
  elapsed="$(elapsed_secs "$start")"

  finish="$(reth_db_capture "$binary" stage-checkpoints get --stage finish || true)"
  finish_number="$(parse_stage_checkpoint_block <<< "$finish")"
  if [ "$finish_number" != "$expected_number" ]; then
    echo "::error::Finish stage checkpoint is ${finish_number:-unknown}, expected ${expected_number} after unwind"
    echo "$finish"
    exit 1
  fi
  if ! persisted_prerun_head_present "$binary" "$expected_number" "$expected_hash"; then
    echo "::error::Post-prerun head ${expected_number} (${expected_hash}) was not present after unwind"
    print_prerun_persistence_diagnostics "$binary" "$expected_number"
    exit 1
  fi
  echo "Unwind reset completed in ${elapsed}s"
}

stop_node() {
  if [ -n "${TAIL_PID:-}" ]; then
    kill "$TAIL_PID" 2>/dev/null || true
    TAIL_PID=""
  fi
  sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
  sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
}

stop_node_required() {
  if [ -n "${TAIL_PID:-}" ]; then
    kill "$TAIL_PID" 2>/dev/null || true
    TAIL_PID=""
  fi

  if sudo systemctl list-units --all --full --no-legend "$RETH_SCOPE" | grep -q "$RETH_SCOPE"; then
    if ! sudo systemctl stop "$RETH_SCOPE"; then
      echo "::error::Failed to stop reth systemd scope ${RETH_SCOPE}"
      sudo systemctl status "$RETH_SCOPE" --no-pager || true
      exit 1
    fi
  fi
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

reset_to_prerun_state() {
  local binary="$1"
  local strategy
  strategy="$(reset_strategy)"

  case "$strategy" in
    schelk)
      recover_schelk
      ;;
    unwind)
      stop_node_required
      unwind_to_prerun_head "$binary"
      drop_caches
      ;;
  esac
}

finalize_run_state() {
  local binary="$1"
  local strategy
  strategy="$(reset_strategy)"

  case "$strategy" in
    schelk)
      sudo schelk recover -y --kill || true
      ;;
    unwind)
      stop_node_required
      unwind_to_prerun_head "$binary"
      ;;
  esac
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
  ensure_jwt_secret

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
    --authrpc.jwtsecret "$JWT_SECRET"
    --disable-discovery
    --no-persist-peers
  )

  if [ -f "$DATADIR/genesis.json" ]; then
    reth_args+=(--chain "$DATADIR/genesis.json")
  fi

  local node_help
  node_help="$("$binary" node --help 2>/dev/null || true)"

  if grep -qF -- '--debug.startup-sync-state-idle' <<< "$node_help"; then
    reth_args+=(--debug.startup-sync-state-idle)
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
    --jwt-secret "$JWT_SECRET" \
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
    "BENCH_RESET_STRATEGY=${BENCH_RESET_STRATEGY:-schelk}" \
    "BENCH_PRERUN_HEAD_FILE=${BENCH_PRERUN_HEAD_FILE:-}" \
    "BENCH_BASELINE_ARGS=${BENCH_BASELINE_ARGS:-}" \
    "BENCH_FEATURE_ARGS=${BENCH_FEATURE_ARGS:-}" \
    "BENCH_METRICS_ADDR=${BENCH_METRICS_ADDR:-}" \
    nice -n -20 "$BENCHMARKOOR_BIN" "$@"
}

shell_command() {
  printf '%q ' "$@"
}

benchmarkoor_failure_status() {
  local native_out="$1"
  if grep -q 'newPayload status is INVALID' "$native_out"; then
    echo "invalid_newpayload"
  else
    echo "error"
  fi
}

benchmarkoor_failure_phase() {
  local native_out="$1"
  if grep -q '/setup/' "$native_out"; then
    echo "setup"
  elif grep -q '/testing/' "$native_out"; then
    echo "testing"
  else
    echo "unknown"
  fi
}

benchmarkoor_error_summary() {
  local native_out="$1"
  { grep -E 'newPayload status is|JSON-RPC error|Error:|Caused by:' "$native_out" || true; } |
    tail -n 12 |
    sed -E 's/[[:space:]]+/ /g' |
    paste -sd ' ' - |
    cut -c1-1200
}

append_benchmarkoor_failure_result() {
  local native_out="$1"
  local test_name="$2"
  local gas_bucket="$3"
  local label="$4"
  local run_type="$5"
  local strategy="$6"
  local reset_elapsed_secs="$7"
  local wall_elapsed_secs="$8"
  local benchmarkoor_status="$9"
  local results="${10}"

  local status phase error_summary
  status="$(benchmarkoor_failure_status "$native_out")"
  phase="$(benchmarkoor_failure_phase "$native_out")"
  error_summary="$(benchmarkoor_error_summary "$native_out")"
  if [ -z "$error_summary" ]; then
    error_summary="benchmarkoor-replay exited with status ${benchmarkoor_status}"
  fi

  jq -nc \
    --arg suite "$BENCHMARKOOR_SUITE" \
    --arg test "$test_name" \
    --arg label "$label" \
    --arg run_type "$run_type" \
    --arg gas_bucket "$gas_bucket" \
    --arg reset_strategy "$strategy" \
    --arg status "$status" \
    --arg phase "$phase" \
    --arg error_summary "$error_summary" \
    --argjson reset_elapsed_secs "$reset_elapsed_secs" \
    --argjson wall_elapsed_secs "$wall_elapsed_secs" \
    --argjson benchmarkoor_exit_code "$benchmarkoor_status" \
    '{
      kind: "run_many_result",
      status: $status,
      suite: $suite,
      test: $test,
      label: $label,
      run_type: $run_type,
      gas_bucket: $gas_bucket,
      reset_strategy: $reset_strategy,
      reset_elapsed_secs: $reset_elapsed_secs,
      wall_elapsed_secs: $wall_elapsed_secs,
      benchmarkoor_exit_code: $benchmarkoor_exit_code,
      invalid_newpayload: ($status == "invalid_newpayload"),
      invalid_newpayload_phase: (if $status == "invalid_newpayload" then $phase else null end),
      error_summary: $error_summary,
      gas: 0,
      elapsed_ms: 0,
      gas_per_second: null,
      testing_gas_per_sec: null,
      testing_gas_used: 0,
      testing_elapsed: null
    }' >> "$results"
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
  strategy="$(reset_strategy)"

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

  stop_node_required
  wait_for_persisted_prerun_head "$binary" true
  if [ "$strategy" = "schelk" ]; then
    sudo schelk promote -y
    echo "Promoted schelk baseline after benchmarkoor gas-bump/funding"
  else
    echo "Prepared unwind baseline after benchmarkoor gas-bump/funding"
  fi
  sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'
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

strategy="$(reset_strategy)"
mapfile -d '' common < <(benchmarkoor_common_args "$binary" "schelk")
run_type="feature"
if [[ "$label" == baseline* ]]; then
  run_type="baseline"
fi

selected_count="$(grep -cve '^[[:space:]]*$' "$tests_jsonl" || true)"
completed_count=0
recorded_failure_count=0

while IFS= read -r test_entry; do
  [ -n "$test_entry" ] || continue
  test_name="$(jq -r '.name' <<< "$test_entry")"
  gas_bucket="$(jq -r '.gas_bucket // ""' <<< "$test_entry")"
  test_slug="$(safe_name "$test_name")"
  log_path="${output_dir}/logs/$(safe_name "${label}-${test_slug}").log"
  native_out="${output_dir}/native-$(safe_name "$test_slug").jsonl"
  before_count="$(wc -l < "$results" | tr -d ' ')"
  test_start_ns="$(now_ns)"
  reset_start_ns="$(now_ns)"

  echo "=== ${label}: ${test_name} ==="
  reset_to_prerun_state "$binary"
  reset_elapsed_secs="$(elapsed_secs "$reset_start_ns")"
  start_node "$binary" "$label" "$output_dir" "${test_slug}-setup" "$log_path" false true

  restart_command="$(
    shell_command "$SCRIPT_PATH" restart-node "$binary" "$label" "$output_dir" "${test_slug}-testing" "$log_path"
  )"

  set +e
  run_benchmarkoor "${common[@]}" run-many \
    --exact "$test_name" \
    --limit 1 \
    --repetitions 1 \
    --mode setup-then-testing \
    --no-schelk \
    --drop-caches \
    --restart-node-command "$restart_command" \
    --json 2>&1 | tee "$native_out"
  pipeline_status=("${PIPESTATUS[@]}")
  set -e
  benchmarkoor_status="${pipeline_status[0]}"
  tee_status="${pipeline_status[1]}"
  if [ "$tee_status" -ne 0 ]; then
    echo "::error::tee failed while recording benchmarkoor output for ${test_name}"
    exit 1
  fi

  wall_elapsed_secs="$(elapsed_secs "$test_start_ns")"
  echo "=== ${label}: ${test_name} completed in ${wall_elapsed_secs}s (reset=${reset_elapsed_secs}s, strategy=${strategy}) ==="

  jq -Rnc \
    --arg label "$label" \
    --arg run_type "$run_type" \
    --arg gas_bucket "$gas_bucket" \
    --arg reset_strategy "$strategy" \
    --argjson reset_elapsed_secs "$reset_elapsed_secs" \
    --argjson wall_elapsed_secs "$wall_elapsed_secs" \
    'inputs | fromjson? | select(.kind == "run_many_result") |
      . + {
        status: (.status // "ok"),
        invalid_newpayload: (.invalid_newpayload // false),
        label: $label,
        run_type: $run_type,
        gas_bucket: $gas_bucket,
        reset_strategy: $reset_strategy,
        reset_elapsed_secs: $reset_elapsed_secs,
        wall_elapsed_secs: $wall_elapsed_secs
      }' \
    "$native_out" >> "$results"

  after_count="$(wc -l < "$results" | tr -d ' ')"
  emitted_count=$(( after_count - before_count ))
  if [ "$benchmarkoor_status" -ne 0 ]; then
    echo "::warning::benchmarkoor-replay failed for ${test_name}; recording failure and continuing"
    append_benchmarkoor_failure_result \
      "$native_out" \
      "$test_name" \
      "$gas_bucket" \
      "$label" \
      "$run_type" \
      "$strategy" \
      "$reset_elapsed_secs" \
      "$wall_elapsed_secs" \
      "$benchmarkoor_status" \
      "$results"
    recorded_failure_count=$(( recorded_failure_count + 1 ))
  elif [ "$emitted_count" -le 0 ]; then
    echo "::warning::benchmarkoor-replay did not emit a run_many_result for ${test_name}; recording failure and continuing"
    append_benchmarkoor_failure_result \
      "$native_out" \
      "$test_name" \
      "$gas_bucket" \
      "$label" \
      "$run_type" \
      "$strategy" \
      "$reset_elapsed_secs" \
      "$wall_elapsed_secs" \
      0 \
      "$results"
    recorded_failure_count=$(( recorded_failure_count + 1 ))
  fi

  completed_count=$(( completed_count + 1 ))
done < "$tests_jsonl"

echo "Completed ${completed_count}/${selected_count} selected benchmarkoor test(s) for ${label}; recorded ${recorded_failure_count} replay failure(s)"
finalize_run_state "$binary"
echo "results=${results}"
