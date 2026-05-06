#!/usr/bin/env bash

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: repro-hoodi-partial-persistence-reorg.sh [options]

Restores a hoodi datadir snapshot, starts reth with partial persistence, then
runs reth-bench new-payload-fcu with --reorg until a state-root mismatch is
observed, the benchmark exits, or an optional timeout is reached.

Unlike repro-hoodi-partial-persistence-unwind.sh, this script does not crash the
node and does not run restart, unwind, or Merkle-stage follow-up steps.

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
  --to-block N                Last block to replay before declaring no mismatch
                              (default: unset, run continuously from restored head)
  --reorg-depth N             reth-bench --reorg depth
                              (default: 8)
  --artifacts-dir PATH        Directory for logs and summary output
                              (default: /tmp/reth-hoodi-reorg-<timestamp>)
  --start-timeout SECONDS     Seconds to wait for node RPC startup
                              (default: 180)
  --mismatch-timeout SECONDS  Seconds to wait for a state-root mismatch after
                              reth-bench starts (default: 0, no timeout)
  -h, --help                  Show this help

Exit codes:
  0  Script ran to completion. See result.txt for whether a state-root mismatch
     was observed.
  2  Setup/runtime failure prevented a conclusive run.
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
    local label="$2"

    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        log "Sending SIGTERM to ${label} (pid ${pid})"
        kill -TERM "$pid" 2>/dev/null || true
    fi
}

stop_matching_processes() {
    local pattern="$1"
    local label="$2"
    local -a pids=()
    local pid

    while IFS= read -r pid; do
        [[ -n "$pid" ]] && pids+=("$pid")
    done < <(pgrep -f "$pattern" || true)

    if ((${#pids[@]} > 0)); then
        log "Sending SIGTERM to stale ${label} processes: ${pids[*]}"
        kill -TERM "${pids[@]}" 2>/dev/null || true
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
        printf 'to_block=%s\n' "${TO_BLOCK:-unset}"
        printf 'reorg_depth=%s\n' "$REORG_DEPTH"
        printf 'head_before=%s\n' "${HEAD_BEFORE:-unknown}"
        printf 'head_after=%s\n' "${HEAD_AFTER:-unknown}"
        printf 'bench_exit_code=%s\n' "${BENCH_EXIT_CODE:-unknown}"
        printf 'mismatch_source=%s\n' "${MISMATCH_SOURCE:-not_found}"
        printf 'mismatch_line=%s\n' "${MISMATCH_LINE:-not_found}"
        printf 'artifacts_dir=%s\n' "$ARTIFACTS_DIR"
        printf 'node_log=%s\n' "$NODE_LOG"
        printf 'bench_log=%s\n' "$BENCH_LOG"
    } >"$SUMMARY_FILE"
}

cleanup() {
    stop_pid "${BENCH_PID:-}" "reth-bench"
    if [[ -n "${BENCH_PID:-}" ]]; then
        wait "${BENCH_PID}" 2>/dev/null || true
    fi

    stop_pid "${NODE_PID:-}" "reth node"
    if [[ -n "${NODE_PID:-}" ]]; then
        wait "${NODE_PID}" 2>/dev/null || true
    fi

    write_summary
}

SNAPSHOT="/mnt/data/hoodi.tar.zst"
DATADIR="/mnt/data/hoodi"
JWT_SECRET=""
REMOTE_RPC_URL="https://rpc.hoodi.ethpandaops.io"
EXPECTED_HEAD=2613962
START_BLOCK=2613963
TO_BLOCK=""
REORG_DEPTH=8
START_TIMEOUT=180
MISMATCH_TIMEOUT=0
RETH_BIN="/repos/reth/target/profiling/reth"
BENCH_BIN="/repos/reth/target/profiling/reth-bench"
CHAIN="hoodi"
RESULT="script_error"
HEAD_BEFORE=""
HEAD_AFTER=""
NODE_PID=""
BENCH_PID=""
BENCH_EXIT_CODE=""
MISMATCH_SOURCE=""
MISMATCH_LINE=""
TIMESTAMP="$(date '+%Y%m%d-%H%M%S')"
ARTIFACTS_DIR="/tmp/reth-hoodi-reorg-${TIMESTAMP}"

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
        --to-block)
            TO_BLOCK="$2"
            shift 2
            ;;
        --reorg-depth)
            REORG_DEPTH="$2"
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
        --mismatch-timeout)
            MISMATCH_TIMEOUT="$2"
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
NODE_LOG="${ARTIFACTS_DIR}/node.log"
BENCH_LOG="${ARTIFACTS_DIR}/bench.log"

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

if (( REORG_DEPTH <= 0 )); then
    log "--reorg-depth must be greater than 0"
    exit 2
fi

if [[ -n "$TO_BLOCK" ]] && (( TO_BLOCK < START_BLOCK )); then
    log "--to-block ${TO_BLOCK} must be greater than or equal to --start-block ${START_BLOCK}"
    exit 2
fi

NODE_PATTERN="^$(regex_escape "$RETH_BIN") node --datadir $(regex_escape "$DATADIR")( |$)"
stop_matching_processes "$NODE_PATTERN" "reth"
sleep 1

capture_command reth "$RETH_BIN" node \
    --datadir "$DATADIR" \
    --chain "$CHAIN" \
    --http --http.addr 127.0.0.1 --http.port 8545 --http.api eth,net,web3,reth,testing \
    --ws --ws.addr 127.0.0.1 --ws.port 8546 --ws.api eth,net,web3,reth \
    --authrpc.addr 127.0.0.1 --authrpc.port 8551 --authrpc.jwtsecret "$JWT_SECRET" \
    --disable-discovery \
    --engine.persistence-threshold 10 \
    --engine.deferred-trie-blocks 3 \
    --engine.accept-execution-requests-hash \
    --log.stdout.filter 'info,providers::state::overlay=debug,chain_state::lazy_overlay=debug,engine::tree::payload_validator=debug,providers::db=debug,reth::providers::static_file=debug,reth::storage=debug,consensus::engine=debug,reth-bench=debug' \
    --color never

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
    "$RETH_BIN" node \
        --datadir "$DATADIR" \
        --chain "$CHAIN" \
        --http --http.addr 127.0.0.1 --http.port 8545 --http.api eth,net,web3,reth,testing \
        --ws --ws.addr 127.0.0.1 --ws.port 8546 --ws.api eth,net,web3,reth \
        --authrpc.addr 127.0.0.1 --authrpc.port 8551 --authrpc.jwtsecret "$JWT_SECRET" \
        --disable-discovery \
        --engine.persistence-threshold 10 \
        --engine.deferred-trie-blocks 3 \
        --engine.accept-execution-requests-hash \
        --log.stdout.filter 'info,providers::state::overlay=debug,providers::historical_sp=debug,chain_state::lazy_overlay=debug,chain_state::memory_overlay=debug,engine::tree=debug,engine::tree::payload_validator=debug,payload_builder=debug,providers::db=debug,reth::providers::static_file=debug,reth::storage=debug,consensus::engine=debug,reth-bench=debug' \
        --color never \
        >"$NODE_LOG" 2>&1 &
    echo $!
}

wait_for_rpc_start() {
    local pid="$1"
    local timeout="$2"
    local elapsed=0
    local block_hex

    while (( elapsed < timeout )); do
        block_hex=$(head_hex || true)
        if [[ -n "$block_hex" ]]; then
            printf '%s\n' "$block_hex"
            return 0
        fi

        if ! kill -0 "$pid" 2>/dev/null; then
            log "Node exited before RPC became ready"
            return 1
        fi

        sleep 1
        ((elapsed += 1))
    done

    log "Timed out waiting for node RPC readiness"
    return 1
}

remove_stale_locks() {
    rm -f "$DATADIR/db/lock" "$DATADIR/static_files/lock" "$DATADIR/rocksdb/LOCK"
}

find_mismatch() {
    local source="$1"
    local log_file="$2"
    local line

    line=$(grep -Ei -m1 \
        'state[ -]?root.*mismatch|mismatch.*state[ -]?root|mismatched block state root|Failed to verify block state root' \
        "$log_file" 2>/dev/null || true)
    if [[ -n "$line" ]]; then
        MISMATCH_SOURCE="$source"
        MISMATCH_LINE="$line"
        RESULT="state_root_mismatch"
        return 0
    fi

    return 1
}

monitor_for_mismatch() {
    local start_epoch
    local elapsed
    local block_hex

    start_epoch=$(date +%s)
    while true; do
        if find_mismatch node "$NODE_LOG" || find_mismatch bench "$BENCH_LOG"; then
            log "Observed state-root mismatch in ${MISMATCH_SOURCE} log"
            return 0
        fi

        block_hex=$(head_hex || true)
        if [[ -n "$block_hex" ]]; then
            HEAD_AFTER=$(hex_to_dec "$block_hex")
        fi

        if [[ -n "$BENCH_PID" ]] && ! kill -0 "$BENCH_PID" 2>/dev/null; then
            if wait "$BENCH_PID"; then
                BENCH_EXIT_CODE=0
                RESULT="bench_completed_no_mismatch"
            else
                BENCH_EXIT_CODE=$?
                if find_mismatch node "$NODE_LOG" || find_mismatch bench "$BENCH_LOG"; then
                    log "Observed state-root mismatch after reth-bench exit"
                    return 0
                fi
                RESULT="bench_failed_no_mismatch"
            fi
            BENCH_PID=""
            return 0
        fi

        if [[ -n "$NODE_PID" ]] && ! kill -0 "$NODE_PID" 2>/dev/null; then
            if find_mismatch node "$NODE_LOG" || find_mismatch bench "$BENCH_LOG"; then
                log "Observed state-root mismatch after node exit"
                return 0
            fi
            RESULT="node_exited_no_mismatch"
            return 0
        fi

        if (( MISMATCH_TIMEOUT > 0 )); then
            elapsed=$(($(date +%s) - start_epoch))
            if (( elapsed >= MISMATCH_TIMEOUT )); then
                RESULT="timeout_no_mismatch"
                return 0
            fi
        fi

        sleep 1
    done
}

restore_snapshot

log "Starting reth for reorg replay run"
NODE_PID=$(start_node)

HEAD_HEX=$(wait_for_rpc_start "$NODE_PID" "$START_TIMEOUT") || exit 2
HEAD_BEFORE=$(hex_to_dec "$HEAD_HEX")
HEAD_AFTER="$HEAD_BEFORE"
printf '%s\n' "$HEAD_BEFORE" >"${ARTIFACTS_DIR}/current_head_before.txt"

if (( HEAD_BEFORE != EXPECTED_HEAD )); then
    log "Expected restored head ${EXPECTED_HEAD}, got ${HEAD_BEFORE}"
    exit 2
fi

if (( HEAD_BEFORE + 1 != START_BLOCK )); then
    log "Expected first replay block ${START_BLOCK}, but restored head implies ${HEAD_BEFORE} -> $((HEAD_BEFORE + 1))"
    exit 2
fi

BENCH_ARGS=(
    "$BENCH_BIN" -vvv new-payload-fcu
    --rpc-url "$REMOTE_RPC_URL"
    --from "$HEAD_BEFORE"
    --jwt-secret "$JWT_SECRET"
    --engine-rpc-url http://127.0.0.1:8551
    --local-rpc-url http://127.0.0.1:8545
    --ws-rpc-url ws://127.0.0.1:8546
    --reorg "$REORG_DEPTH"
)

if [[ -n "$TO_BLOCK" ]]; then
    BENCH_ARGS+=(--to "$TO_BLOCK")
fi

capture_command reth_bench "${BENCH_ARGS[@]}"

if [[ -n "$TO_BLOCK" ]]; then
    log "Running reth-bench with --reorg ${REORG_DEPTH} from block ${START_BLOCK} through ${TO_BLOCK}"
else
    log "Running reth-bench with --reorg ${REORG_DEPTH} continuously from block ${START_BLOCK}"
fi

"${BENCH_ARGS[@]}" >"$BENCH_LOG" 2>&1 &
BENCH_PID=$!

monitor_for_mismatch

log "Reorg repro result: ${RESULT}"
