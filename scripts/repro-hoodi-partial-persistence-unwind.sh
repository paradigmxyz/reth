#!/usr/bin/env bash

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: repro-hoodi-partial-persistence-unwind.sh [options]

Restores a hoodi datadir snapshot, runs a partial-persistence sync replay with
reth-bench, kill -9s the node during the post-target persistence window, then
restarts the node and reports whether restart led to an unwind failure.

Options:
  --snapshot PATH             Tar.zst snapshot to restore
                              (default: /mnt/data/hoodi.tar.zst)
  --datadir PATH              Restored reth datadir
                              (default: /mnt/data/hoodi)
  --jwt-secret PATH           JWT secret path
                              (default: <datadir>/jwt.hex)
  --rpc-url URL               Remote hoodi RPC used by reth-bench
                              (default: https://rpc.hoodi.ethpandaops.io)
  --expected-head N           Expected local head after restore
                              (default: 2613962)
  --start-block N             First block expected to be replayed
                              (default: 2613963)
  --target-block N            Last block to replay before crashing
                              (default: 2614300)
  --artifacts-dir PATH        Directory for logs and summary output
                              (default: /tmp/reth-hoodi-unwind-<timestamp>)
  --start-timeout SECONDS     Seconds to wait for node RPC startup
                              (default: 180)
  --target-timeout SECONDS    Seconds to wait for local head to reach target
                              (default: 900)
  --persistence-timeout SEC   Seconds to wait for a persistence marker after
                              the target head is reached (default: 300)
  --restart-timeout SECONDS   Seconds to classify restart behavior
                              (default: 180)
  -h, --help                  Show this help

Exit codes:
  0  Script ran to completion. See result.txt for whether unwind succeeded,
     failed, or was not triggered.
  2  Setup/runtime failure prevented a conclusive result.
EOF
}

log() {
    printf '[%s] %s\n' "$(date '+%H:%M:%S')" "$*" >&2
}

regex_escape() {
    printf '%s' "$1" | sed 's/[][(){}.^$+*?|\\/]/\\&/g'
}

head_hex() {
    local response
    response=$(curl -fsS \
        -H 'Content-Type: application/json' \
        -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
        http://127.0.0.1:8545 2>/dev/null) || return 1
    response=${response//$'\n'/}
    sed -n 's/.*"result"[[:space:]]*:[[:space:]]*"\(0x[0-9a-fA-F]\+\)".*/\1/p' <<<"$response"
}

hex_to_dec() {
    printf '%d\n' "$((16#${1#0x}))"
}

wait_for_pid_exit() {
    local pid="$1"
    local timeout="$2"
    local elapsed=0

    while (( elapsed < timeout )); do
        if ! kill -0 "$pid" 2>/dev/null; then
            return 0
        fi
        sleep 1
        ((elapsed += 1))
    done

    return 1
}

stop_pid() {
    local pid="$1"
    local signal="$2"
    local label="$3"

    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        log "Sending SIG${signal} to ${label} (pid ${pid})"
        kill "-${signal}" "$pid" 2>/dev/null || true
    fi
}

kill_matching_processes() {
    local signal="$1"
    local pattern="$2"
    local label="$3"
    local -a pids=()
    local pid

    while IFS= read -r pid; do
        [[ -n "$pid" ]] && pids+=("$pid")
    done < <(pgrep -f "$pattern" || true)

    if ((${#pids[@]} > 0)); then
        log "Sending SIG${signal} to stale ${label} processes: ${pids[*]}"
        kill "-${signal}" "${pids[@]}" 2>/dev/null || true
    fi
}

capture_command() {
    local name="$1"
    shift
    {
        printf '%s=' "$name"
        printf '%q ' "$@"
        printf '\n'
    } >>"$COMMANDS_FILE"
}

write_summary() {
    {
        printf 'result=%s\n' "${RESULT:-unknown}"
        printf 'snapshot=%s\n' "$SNAPSHOT"
        printf 'datadir=%s\n' "$DATADIR"
        printf 'jwt_secret=%s\n' "$JWT_SECRET"
        printf 'remote_rpc_url=%s\n' "$REMOTE_RPC_URL"
        printf 'expected_head=%s\n' "$EXPECTED_HEAD"
        printf 'start_block=%s\n' "$START_BLOCK"
        printf 'target_block=%s\n' "$TARGET_BLOCK"
        printf 'advance=%s\n' "${ADVANCE:-unknown}"
        printf 'head_before=%s\n' "${HEAD_BEFORE:-unknown}"
        printf 'head_after_crash=%s\n' "${HEAD_AT_CRASH:-unknown}"
        printf 'head_after_restart=%s\n' "${HEAD_AFTER_RESTART:-unknown}"
        printf 'artifacts_dir=%s\n' "$ARTIFACTS_DIR"
        printf 'node1_log=%s\n' "$NODE1_LOG"
        printf 'bench_log=%s\n' "$BENCH_LOG"
        printf 'node2_log=%s\n' "$NODE2_LOG"
        printf 'restart_trace_log=%s\n' "$RESTART_TRACE_LOG"
        printf 'failed_unwind_target=%s\n' "${FAILED_UNWIND_TARGET:-unknown}"
        printf 'drop_merkle_result=%s\n' "${DROP_MERKLE_RESULT:-not_run}"
        printf 'drop_merkle_log=%s\n' "$DROP_MERKLE_LOG"
        printf 'post_drop_unwind_result=%s\n' "${POST_DROP_UNWIND_RESULT:-not_run}"
        printf 'post_drop_unwind_log=%s\n' "$POST_DROP_UNWIND_LOG"
        printf 'post_drop_merkle_run_result=%s\n' "${POST_DROP_MERKLE_RUN_RESULT:-not_run}"
        printf 'post_drop_merkle_run_log=%s\n' "$POST_DROP_MERKLE_RUN_LOG"
        printf 'post_drop_merkle_run_trace_log=%s\n' "$POST_DROP_MERKLE_RUN_TRACE_LOG"
    } >"$SUMMARY_FILE"
}

cleanup() {
    stop_pid "${BENCH_PID:-}" TERM "reth-bench"
    if [[ -n "${BENCH_PID:-}" ]]; then
        wait "${BENCH_PID}" 2>/dev/null || true
    fi

    stop_pid "${NODE2_PID:-}" TERM "reth restart node"
    if [[ -n "${NODE2_PID:-}" ]]; then
        wait "${NODE2_PID}" 2>/dev/null || true
    fi

    stop_pid "${NODE1_PID:-}" TERM "reth crash node"
    if [[ -n "${NODE1_PID:-}" ]]; then
        wait "${NODE1_PID}" 2>/dev/null || true
    fi

    write_summary
}

SNAPSHOT="/mnt/data/hoodi.tar.zst"
DATADIR="/mnt/data/hoodi"
JWT_SECRET=""
REMOTE_RPC_URL="https://rpc.hoodi.ethpandaops.io"
EXPECTED_HEAD=2613962
START_BLOCK=2613963
TARGET_BLOCK=2614300
START_TIMEOUT=180
TARGET_TIMEOUT=900
PERSISTENCE_TIMEOUT=300
RESTART_TIMEOUT=180
RETH_BIN="/repos/reth/target/profiling/reth"
BENCH_BIN="/repos/reth/target/profiling/reth-bench"
CHAIN="hoodi"
MERKLE_TRACE_FILTER='info'
RESULT="script_error"
ADVANCE=""
HEAD_BEFORE=""
HEAD_AT_CRASH=""
HEAD_AFTER_RESTART=""
NODE1_PID=""
NODE2_PID=""
BENCH_PID=""
FAILED_UNWIND_TARGET=""
DROP_MERKLE_RESULT="not_run"
POST_DROP_UNWIND_RESULT="not_run"
POST_DROP_MERKLE_RUN_RESULT="not_run"
TIMESTAMP="$(date '+%Y%m%d-%H%M%S')"
ARTIFACTS_DIR="/tmp/reth-hoodi-unwind-${TIMESTAMP}"

while (($# > 0)); do
    case "$1" in
        --snapshot)
            SNAPSHOT="$2"
            shift 2
            ;;
        --datadir)
            DATADIR="$2"
            shift 2
            ;;
        --jwt-secret)
            JWT_SECRET="$2"
            shift 2
            ;;
        --rpc-url)
            REMOTE_RPC_URL="$2"
            shift 2
            ;;
        --expected-head)
            EXPECTED_HEAD="$2"
            shift 2
            ;;
        --start-block)
            START_BLOCK="$2"
            shift 2
            ;;
        --target-block)
            TARGET_BLOCK="$2"
            shift 2
            ;;
        --artifacts-dir)
            ARTIFACTS_DIR="$2"
            shift 2
            ;;
        --start-timeout)
            START_TIMEOUT="$2"
            shift 2
            ;;
        --target-timeout)
            TARGET_TIMEOUT="$2"
            shift 2
            ;;
        --persistence-timeout)
            PERSISTENCE_TIMEOUT="$2"
            shift 2
            ;;
        --restart-timeout)
            RESTART_TIMEOUT="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown argument: $1" >&2
            usage
            exit 2
            ;;
    esac
done

if [[ -z "$JWT_SECRET" ]]; then
    JWT_SECRET="${DATADIR}/jwt.hex"
fi

mkdir -p "$ARTIFACTS_DIR"
COMMANDS_FILE="${ARTIFACTS_DIR}/commands.txt"
SUMMARY_FILE="${ARTIFACTS_DIR}/result.txt"
NODE1_LOG="${ARTIFACTS_DIR}/node1.log"
BENCH_LOG="${ARTIFACTS_DIR}/bench.log"
NODE2_LOG="${ARTIFACTS_DIR}/node2.log"
RESTART_TRACE_LOG="${ARTIFACTS_DIR}/restart-trace.log"
DROP_MERKLE_LOG="${ARTIFACTS_DIR}/drop-merkle.log"
POST_DROP_UNWIND_LOG="${ARTIFACTS_DIR}/post-drop-unwind.log"
POST_DROP_MERKLE_RUN_LOG="${ARTIFACTS_DIR}/post-drop-merkle-run.log"
POST_DROP_MERKLE_RUN_TRACE_LOG="not_captured"

trap cleanup EXIT

if [[ ! -x "$RETH_BIN" ]]; then
    log "Missing executable reth binary: $RETH_BIN"
    exit 2
fi

if [[ ! -x "$BENCH_BIN" ]]; then
    log "Missing executable reth-bench binary: $BENCH_BIN"
    exit 2
fi

if [[ ! -f "$SNAPSHOT" ]]; then
    log "Missing snapshot archive: $SNAPSHOT"
    exit 2
fi

NODE_PATTERN="^$(regex_escape "$RETH_BIN") node --datadir $(regex_escape "$DATADIR")( |$)"
kill_matching_processes TERM "$NODE_PATTERN" "reth"
sleep 1
kill_matching_processes KILL "$NODE_PATTERN" "reth"

capture_command reth "$RETH_BIN" node \
    --datadir "$DATADIR" \
    --chain "$CHAIN" \
    --http --http.addr 127.0.0.1 --http.port 8545 --http.api eth,net,web3,reth \
    --ws --ws.addr 127.0.0.1 --ws.port 8546 --ws.api eth,net,web3,reth \
    --authrpc.addr 127.0.0.1 --authrpc.port 8551 --authrpc.jwtsecret "$JWT_SECRET" \
    --disable-discovery \
    --engine.persistence-threshold 10 \
    --engine.deferred-trie-blocks 3 \
    --engine.accept-execution-requests-hash \
    --log.stdout.filter 'info,providers::db=debug,reth::providers::static_file=debug,reth::storage=debug,consensus::engine=debug' \
    --color never

capture_command reth_restart "$RETH_BIN" node \
    --datadir "$DATADIR" \
    --chain "$CHAIN" \
    --http --http.addr 127.0.0.1 --http.port 8545 --http.api eth,net,web3,reth \
    --ws --ws.addr 127.0.0.1 --ws.port 8546 --ws.api eth,net,web3,reth \
    --authrpc.addr 127.0.0.1 --authrpc.port 8551 --authrpc.jwtsecret "$JWT_SECRET" \
    --disable-discovery \
    --engine.persistence-threshold 10 \
    --engine.deferred-trie-blocks 3 \
    --engine.accept-execution-requests-hash \
    --log.stdout.filter trace \
    --color never

capture_command reth_stage_drop_merkle "$RETH_BIN" stage drop \
    --datadir "$DATADIR" \
    --chain "$CHAIN" \
    --log.stdout.filter info \
    --color never \
    merkle

restore_snapshot() {
    local parent_dir
    local base_name
    local extract_root
    local candidate_datadir=""
    local -a nested_candidates=()
    local nested_dir

    parent_dir=$(dirname "$DATADIR")
    base_name=$(basename "$DATADIR")
    extract_root="${DATADIR}.extract.$$"

    log "Restoring snapshot ${SNAPSHOT} into ${DATADIR}"
    rm -rf "$DATADIR" "$extract_root"
    mkdir -p "$parent_dir" "$extract_root"
    tar --zstd -xf "$SNAPSHOT" -C "$extract_root"

    if [[ -d "${extract_root}/${base_name}/db" && -d "${extract_root}/${base_name}/static_files" ]]; then
        candidate_datadir="${extract_root}/${base_name}"
    else
        while IFS= read -r nested_dir; do
            if [[ -d "${nested_dir}/db" && -d "${nested_dir}/static_files" ]]; then
                nested_candidates+=("$nested_dir")
            fi
        done < <(find "$extract_root" -mindepth 1 -maxdepth 1 -type d | sort)

        if ((${#nested_candidates[@]} == 1)); then
            candidate_datadir="${nested_candidates[0]}"
        elif ((${#nested_candidates[@]} > 1)); then
            log "Snapshot layout produced multiple nested datadir candidates under ${extract_root}: ${nested_candidates[*]}"
            exit 2
        elif [[ -d "${extract_root}/db" && -d "${extract_root}/static_files" ]]; then
            candidate_datadir="$extract_root"
        fi
    fi

    if [[ -z "$candidate_datadir" ]]; then
        log "Snapshot layout did not produce an expected datadir under ${extract_root}"
        exit 2
    fi

    if [[ "$candidate_datadir" == "$extract_root" ]]; then
        mv "$extract_root" "$DATADIR"
    else
        mv "$candidate_datadir" "$DATADIR"
        rm -rf "$extract_root"
    fi

    if [[ ! -f "$JWT_SECRET" ]]; then
        log "Restored datadir is missing jwt secret; generating ${JWT_SECRET}"
        mkdir -p "$(dirname "$JWT_SECRET")"
        umask 077
        head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$JWT_SECRET"
        printf '\n' >>"$JWT_SECRET"
    fi
}

start_node() {
    local log_file="$1"

    "$RETH_BIN" node \
        --datadir "$DATADIR" \
        --chain "$CHAIN" \
        --http --http.addr 127.0.0.1 --http.port 8545 --http.api eth,net,web3,reth \
        --ws --ws.addr 127.0.0.1 --ws.port 8546 --ws.api eth,net,web3,reth \
        --authrpc.addr 127.0.0.1 --authrpc.port 8551 --authrpc.jwtsecret "$JWT_SECRET" \
        --disable-discovery \
        --engine.persistence-threshold 10 \
        --engine.deferred-trie-blocks 3 \
        --engine.accept-execution-requests-hash \
        --log.stdout.filter 'info,providers::db=debug,reth::providers::static_file=debug,reth::storage=debug,consensus::engine=debug' \
        --color never \
        >"$log_file" 2>&1 &
    echo $!
}

start_unwind_node() {
    local log_file="$1"
    local trace_log="$2"

    "$RETH_BIN" node \
        --datadir "$DATADIR" \
        --chain "$CHAIN" \
        --http --http.addr 127.0.0.1 --http.port 8545 --http.api eth,net,web3,reth \
        --ws --ws.addr 127.0.0.1 --ws.port 8546 --ws.api eth,net,web3,reth \
        --authrpc.addr 127.0.0.1 --authrpc.port 8551 --authrpc.jwtsecret "$JWT_SECRET" \
        --disable-discovery \
        --engine.persistence-threshold 10 \
        --engine.deferred-trie-blocks 3 \
        --engine.accept-execution-requests-hash \
        --log.stdout.filter trace \
        --color never \
        > >(tee "$trace_log" >"$log_file") 2>&1 &
    echo $!
}

wait_for_rpc_start() {
    local pid="$1"
    local timeout="$2"
    local label="$3"
    local elapsed=0
    local block_hex

    while (( elapsed < timeout )); do
        block_hex=$(head_hex || true)
        if [[ -n "$block_hex" ]]; then
            printf '%s\n' "$block_hex"
            return 0
        fi

        if ! kill -0 "$pid" 2>/dev/null; then
            log "${label} exited before RPC became ready"
            return 1
        fi

        sleep 1
        ((elapsed += 1))
    done

    log "Timed out waiting for ${label} RPC readiness"
    return 1
}

wait_for_target_head() {
    local pid="$1"
    local target="$2"
    local timeout="$3"
    local elapsed=0
    local block_hex
    local block_dec

    while (( elapsed < timeout )); do
        block_hex=$(head_hex || true)
        if [[ -n "$block_hex" ]]; then
            block_dec=$(hex_to_dec "$block_hex")
            if (( block_dec >= target )); then
                printf '%s\n' "$block_dec"
                return 0
            fi
        fi

        if ! kill -0 "$pid" 2>/dev/null; then
            log "Node exited before reaching target head ${target}"
            return 1
        fi

        sleep 1
        ((elapsed += 1))
    done

    log "Timed out waiting for local head to reach ${target}"
    return 1
}

wait_for_persistence_marker() {
    local pid="$1"
    local log_file="$2"
    local start_line="$3"
    local timeout="$4"
    local elapsed=0

    while (( elapsed <= timeout )); do
        if grep -E -m1 \
            'save_blocks step plan|save_blocks trie paths|write_trie_updates|Persisting canonical chain|Appended block data range' \
            < <(tail -n "+${start_line}" "$log_file") \
            >/dev/null 2>&1; then
            return 0
        fi

        if ! kill -0 "$pid" 2>/dev/null; then
            log "Node exited before emitting a post-target persistence marker"
            return 1
        fi

        sleep 1
        ((elapsed += 1))
    done

    log "Timed out waiting for a post-target persistence marker"
    return 1
}

stop_bench() {
    if [[ -n "$BENCH_PID" ]] && kill -0 "$BENCH_PID" 2>/dev/null; then
        stop_pid "$BENCH_PID" TERM "reth-bench"
        if ! wait_for_pid_exit "$BENCH_PID" 10; then
            stop_pid "$BENCH_PID" KILL "reth-bench"
            wait "$BENCH_PID" 2>/dev/null || true
        else
            wait "$BENCH_PID" 2>/dev/null || true
        fi
    elif [[ -n "$BENCH_PID" ]]; then
        wait "$BENCH_PID" 2>/dev/null || true
    fi

    BENCH_PID=""
}

remove_stale_locks() {
    rm -f "$DATADIR/db/lock" "$DATADIR/static_files/lock" "$DATADIR/rocksdb/LOCK"
}

stop_restart_node() {
    if [[ -n "$NODE2_PID" ]] && kill -0 "$NODE2_PID" 2>/dev/null; then
        stop_pid "$NODE2_PID" TERM "reth restart node"
        if ! wait_for_pid_exit "$NODE2_PID" 30; then
            stop_pid "$NODE2_PID" KILL "reth restart node"
        fi
    fi

    if [[ -n "$NODE2_PID" ]]; then
        wait "$NODE2_PID" 2>/dev/null || true
    fi

    NODE2_PID=""
}

extract_unwind_target() {
    local log_file="$1"

    sed -n 's/.*unwind_target=Unwind(\([0-9]\+\)).*/\1/p' "$log_file" | head -n1
}

run_drop_merkle() {
    log "Dropping the Merkle stage before the targeted unwind rerun"
    "$RETH_BIN" stage drop \
        --datadir "$DATADIR" \
        --chain "$CHAIN" \
        --log.stdout.filter info \
        --color never \
        merkle \
        >"$DROP_MERKLE_LOG" 2>&1
}

run_post_drop_unwind() {
    local target="$1"

    log "Re-running unwind without trace capture to restore the pre-failure head"
    "$RETH_BIN" stage unwind \
        --datadir "$DATADIR" \
        --chain "$CHAIN" \
        --log.stdout.filter info \
        --color never \
        to-block "$target" \
        >"$POST_DROP_UNWIND_LOG" 2>&1
}

run_post_drop_merkle() {
    local target="$1"
    local merkle_pid

    log "Rebuilding the Merkle stage without trace capture in ${POST_DROP_MERKLE_RUN_LOG}"
    (
        "$RETH_BIN" stage run \
            --datadir "$DATADIR" \
            --chain "$CHAIN" \
            --from 0 \
            --to "$target" \
            --skip-unwind \
            --checkpoints \
            --commit \
            --disable-discovery \
            --log.stdout.filter "$MERKLE_TRACE_FILTER" \
            --color never \
            merkle \
            >"$POST_DROP_MERKLE_RUN_LOG" 2>&1
    ) &
    merkle_pid=$!

    while kill -0 "$merkle_pid" 2>/dev/null; do
        log "Waiting for the post-drop Merkle rebuild to finish"
        sleep 300
    done

    wait "$merkle_pid"
}

classify_restart() {
    local pid="$1"
    local log_file="$2"
    local timeout="$3"
    local elapsed=0
    local saw_unwind=0
    local rpc_ready_at=-1
    local block_hex

    while (( elapsed < timeout )); do
        if grep -E -q 'Failed to verify block state root|failed to run unwind|mismatched block state root' "$log_file" 2>/dev/null; then
            RESULT="unwind_failed"
            return 0
        fi

        if grep -E -q 'Executing unwind after consistency check|inconsistency_source=partial state trie' "$log_file" 2>/dev/null; then
            saw_unwind=1
        fi

        block_hex=$(head_hex || true)
        if [[ -n "$block_hex" ]]; then
            HEAD_AFTER_RESTART=$(hex_to_dec "$block_hex")
            if (( rpc_ready_at < 0 )); then
                rpc_ready_at=$elapsed
                log "Restart RPC became ready at head ${HEAD_AFTER_RESTART}"
            fi
        fi

        if (( rpc_ready_at >= 0 && elapsed >= rpc_ready_at + 10 )); then
            if (( saw_unwind == 1 )); then
                RESULT="unwind_succeeded"
            else
                RESULT="no_unwind_detected"
            fi
            return 0
        fi

        if ! kill -0 "$pid" 2>/dev/null; then
            if grep -E -q 'Failed to verify block state root|failed to run unwind|mismatched block state root' "$log_file" 2>/dev/null; then
                RESULT="unwind_failed"
                return 0
            fi

            RESULT="restart_exited_before_rpc_ready"
            return 1
        fi

        sleep 1
        ((elapsed += 1))
    done

    RESULT="restart_timeout"
    return 1
}

restore_snapshot

log "Starting reth for replay run"
NODE1_PID=$(start_node "$NODE1_LOG")

HEAD_HEX=$(wait_for_rpc_start "$NODE1_PID" "$START_TIMEOUT" "initial node") || exit 2
HEAD_BEFORE=$(hex_to_dec "$HEAD_HEX")
printf '%s\n' "$HEAD_BEFORE" >"${ARTIFACTS_DIR}/current_head_before.txt"

if (( HEAD_BEFORE != EXPECTED_HEAD )); then
    log "Expected restored head ${EXPECTED_HEAD}, got ${HEAD_BEFORE}"
    exit 2
fi

if (( HEAD_BEFORE + 1 != START_BLOCK )); then
    log "Expected first replay block ${START_BLOCK}, but restored head implies ${HEAD_BEFORE} -> $((HEAD_BEFORE + 1))"
    exit 2
fi

ADVANCE=$((TARGET_BLOCK - HEAD_BEFORE))
if (( ADVANCE <= 0 )); then
    log "Target block ${TARGET_BLOCK} must be greater than restored head ${HEAD_BEFORE}"
    exit 2
fi

capture_command reth_bench "$BENCH_BIN" -vvv new-payload-fcu \
    --rpc-url "$REMOTE_RPC_URL" \
    --advance "$ADVANCE" \
    --jwt-secret "$JWT_SECRET" \
    --engine-rpc-url http://127.0.0.1:8551 \
    --local-rpc-url http://127.0.0.1:8545 \
    --ws-rpc-url ws://127.0.0.1:8546

log "Running reth-bench with --advance ${ADVANCE} so replay begins at block ${START_BLOCK}"
"$BENCH_BIN" -vvv new-payload-fcu \
    --rpc-url "$REMOTE_RPC_URL" \
    --advance "$ADVANCE" \
    --jwt-secret "$JWT_SECRET" \
    --engine-rpc-url http://127.0.0.1:8551 \
    --local-rpc-url http://127.0.0.1:8545 \
    --ws-rpc-url ws://127.0.0.1:8546 \
    >"$BENCH_LOG" 2>&1 &
BENCH_PID=$!

HEAD_AT_CRASH=$(wait_for_target_head "$NODE1_PID" "$TARGET_BLOCK" "$TARGET_TIMEOUT") || exit 2
printf '%s\n' "$HEAD_AT_CRASH" >"${ARTIFACTS_DIR}/current_head_at_crash.txt"
# Allow for a small race where the target-head poll returns after the relevant
# persistence logs were already emitted.
POST_TARGET_LINE=$(( $(wc -l <"$NODE1_LOG") - 50 ))
(( POST_TARGET_LINE < 1 )) && POST_TARGET_LINE=1

log "Target head ${TARGET_BLOCK} reached; sending SIGTERM to trigger the final persistence flush"
stop_pid "$NODE1_PID" TERM "reth crash node"

log "Waiting for the shutdown-triggered persistence marker before crashing"
wait_for_persistence_marker "$NODE1_PID" "$NODE1_LOG" "$POST_TARGET_LINE" "$PERSISTENCE_TIMEOUT" || exit 2

log "Crashing reth with SIGKILL"
stop_pid "$NODE1_PID" KILL "reth crash node"
wait "$NODE1_PID" 2>/dev/null || true
NODE1_PID=""

stop_bench
remove_stale_locks

log "Restarting reth to classify unwind behavior"
NODE2_PID=$(start_unwind_node "$NODE2_LOG" "$RESTART_TRACE_LOG")
classify_restart "$NODE2_PID" "$NODE2_LOG" "$RESTART_TIMEOUT" || exit 2

FAILED_UNWIND_TARGET=$(extract_unwind_target "$NODE2_LOG" || true)
if [[ -n "$FAILED_UNWIND_TARGET" ]]; then
    printf '%s\n' "$FAILED_UNWIND_TARGET" >"${ARTIFACTS_DIR}/failed_unwind_target.txt"
fi

stop_restart_node
remove_stale_locks

if [[ "$RESULT" == "unwind_failed" || "$RESULT" == "unwind_succeeded" ]]; then
    if [[ -z "$FAILED_UNWIND_TARGET" ]]; then
        log "Failed to extract unwind_target from ${NODE2_LOG}"
        exit 2
    fi

    capture_command reth_stage_unwind "$RETH_BIN" stage unwind \
        --datadir "$DATADIR" \
        --chain "$CHAIN" \
        --log.stdout.filter info \
        --color never \
        to-block "$FAILED_UNWIND_TARGET"

    capture_command reth_stage_run_merkle "$RETH_BIN" stage run \
        --datadir "$DATADIR" \
        --chain "$CHAIN" \
        --from 0 \
        --to "$FAILED_UNWIND_TARGET" \
        --skip-unwind \
        --checkpoints \
        --commit \
        --disable-discovery \
        --log.stdout.filter "$MERKLE_TRACE_FILTER" \
        --color never \
        merkle

    if ! run_drop_merkle; then
        DROP_MERKLE_RESULT="failed"
        exit 2
    fi
    DROP_MERKLE_RESULT="ok"

    if ! run_post_drop_unwind "$FAILED_UNWIND_TARGET"; then
        POST_DROP_UNWIND_RESULT="failed"
        exit 2
    fi
    POST_DROP_UNWIND_RESULT="ok"

    remove_stale_locks

    if ! run_post_drop_merkle "$FAILED_UNWIND_TARGET"; then
        POST_DROP_MERKLE_RUN_RESULT="failed"
        exit 2
    fi
    POST_DROP_MERKLE_RUN_RESULT="ok"
else
    DROP_MERKLE_RESULT="skipped_no_unwind_target"
    POST_DROP_UNWIND_RESULT="skipped_no_unwind_target"
    POST_DROP_MERKLE_RUN_RESULT="skipped_no_unwind_target"
fi

log "Restart result: ${RESULT}"
