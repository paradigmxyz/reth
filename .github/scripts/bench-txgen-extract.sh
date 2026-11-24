#!/usr/bin/env bash
#
# Pre-extracts txgen payloads or transactions so they can be reused
# across multiple benchmark runs instead of re-fetching from the remote RPC
# every time.
#
# Starts a throwaway reth node to discover the snapshot's chain tip, extracts
# data from BENCH_RPC_URL, then stops the node and recovers the snapshot.
#
# Usage: bench-txgen-extract.sh <binary> <output-dir>
#
# On success, writes:
#   <output-dir>/all-blocks.ndjson   (or all-big-blocks.ndjson)
#   <output-dir>/warmup-blocks.ndjson
#   <output-dir>/benchmark-blocks.ndjson
# In RPC mode, the corresponding names end in `-transactions.ndjson`.
#
# In call mode it instead writes a request corpus replayed by `bench call`:
#   <output-dir>/corpus.jsonl[.gz]   one JSON-RPC request per line
#   <output-dir>/corpus.meta.json    record counts and the tip it was built for
#
# Corpus sources
# --------------
# `corpus=static` generates the corpus from the chain around the snapshot tip
# for the requested class. `corpus=<NAME>` replays a corpus staged on the runner
# under BENCH_CORPUS_DIR (default /reth-bench/corpora), which holds:
#
#   <name>.jsonl.gz   the corpus itself, gzip optional
#   <name>.sha256     optional checksum, verified before the corpus is used
#   tracers/<name>.js  JavaScript tracers selectable as tracer=js:<name>
#
# Provisioning that directory is manual (copy the file onto the runner). Names
# must not contain path separators. Corpus contents are never logged and never
# uploaded as build artifacts; only the record counts in corpus.meta.json are.
#
# Required env: SCHELK_MOUNT, BENCH_RPC_URL, BENCH_BLOCKS, BENCH_WARMUP_BLOCKS
# Optional env: BENCH_EXECUTION_MODE, BENCH_BIG_BLOCKS, BENCH_BIG_BLOCKS_TARGET_GAS, BENCH_BAL,
#               BENCH_CORPUS, BENCH_CORPUS_DIR, BENCH_CALL_CLASS, BENCH_CALL_METHODS, BENCH_CALL_TOP_GAS,
#               BENCH_CALL_TRACER, BENCH_CALL_TRACER_CONFIG, BENCH_CALL_TRACE_OPTIONS, BENCH_CALL_NAMESPACE
set -euxo pipefail

BINARY="$1"
OUTPUT_DIR="$2"

BIG_BLOCKS="${BENCH_BIG_BLOCKS:-false}"
EXECUTION_MODE="${BENCH_EXECUTION_MODE:-engine}"
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

# Call mode probes the snapshot's tracing range before the node is stopped.
if [ "$EXECUTION_MODE" = "call" ]; then
  RETH_ARGS+=(--http.api "eth,net,web3,debug,trace")
fi

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

HEAD_HASH=$(curl -sf http://127.0.0.1:8545 -X POST \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"eth_getBlockByNumber","params":["latest",false],"id":1}' \
  | jq -r '.result.hash')

# Returns non-zero when the snapshot can no longer trace the given block, which
# happens once the block falls out of the retained account and storage history.
# The archive node serves the blocks after the snapshot tip for the call
# classes; probe it once so an outage falls back instead of failing the run.
archive_rpc_reachable() {
  local response
  response=$(curl -sf -m 15 "$BENCH_RPC_URL" -X POST -H 'Content-Type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' 2>/dev/null) || return 1
  [[ "$response" == *'"result"'* ]]
}

block_is_traceable() {
  local block="$1"
  local block_hex tx_hash response
  block_hex=$(printf '0x%x' "$block")
  tx_hash=$(curl -sf http://127.0.0.1:8545 -X POST \
    -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getBlockByNumber\",\"params\":[\"${block_hex}\",false],\"id\":1}" \
    | jq -r '.result.transactions[0] // empty')
  if [ -z "$tx_hash" ]; then
    echo "Block ${block} has no transactions, skipping traceability probe"
    return 0
  fi
  # The block class never resolves hashes, so it probes the block-addressed
  # method it will replay; the transaction class probes by hash, which also
  # detects a pruned transaction lookup.
  local probe
  if [ "$CALL_CLASS" = "traceblock" ]; then
    probe="{\"jsonrpc\":\"2.0\",\"method\":\"debug_traceBlockByNumber\",\"params\":[\"${block_hex}\",{\"tracer\":\"callTracer\",\"tracerConfig\":{\"onlyTopCall\":true}}],\"id\":1}"
  else
    probe="{\"jsonrpc\":\"2.0\",\"method\":\"debug_traceTransaction\",\"params\":[\"${tx_hash}\",{\"tracer\":\"callTracer\"}],\"id\":1}"
  fi
  if ! response=$(curl -sf http://127.0.0.1:8545 -X POST \
    -H 'Content-Type: application/json' \
    -d "$probe"); then
    echo "traceability probe request for block ${block} failed"
    return 1
  fi
  if jq -e 'has("error")' <<< "$response" > /dev/null 2>&1; then
    local message
    message="$(jq -r '.error.message' <<< "$response")"
    # A transaction the node just listed in the block but cannot find by hash
    # means the transaction lookup index is pruned, so no block in the range
    # will do any better; report that instead of shrinking the range.
    if [[ "$message" == *"transaction not found"* ]]; then
      echo "::error::Snapshot has no transaction hash index (transaction lookup is pruned); hash-addressed classes cannot run on it. Use class=tracecall or class=traceblock, or a snapshot that keeps the transaction lookup."
      exit 1
    fi
    echo "Block ${block} is not traceable: ${message}"
    return 1
  fi
  return 0
}

CALL_BLOCKS="${BENCH_BLOCKS:-20}"
CALL_CLASS="${BENCH_CALL_CLASS:-call}"
CORPUS_SOURCE="${BENCH_CORPUS:-static}"
if [ "$EXECUTION_MODE" = "call" ] && [ "$CORPUS_SOURCE" = "static" ]; then
  case "$CALL_CLASS" in
    tracetx|traceblock)
      # The snapshot is a pruned full node whose persisted state can lag its
      # header tip, so the newest blocks may trace against stale state ("nonce
      # too high"). Walk down from the tip to the newest block that actually
      # traces, then window below it; also shrink the window if its oldest
      # block has fallen past the retained history floor.
      CORPUS_TOP="$HEAD_DEC"
      probe_budget="${BENCH_TRACE_PROBE_DEPTH:-1024}"
      while ! block_is_traceable "$CORPUS_TOP"; do
        probe_budget=$(( probe_budget - 1 ))
        if [ "$probe_budget" -le 0 ] || [ "$CORPUS_TOP" -le 1 ]; then
          echo "::error::Snapshot has no traceable block near its tip ${HEAD_DEC} (searched down to ${CORPUS_TOP}); its persisted state lags too far behind the header for class ${CALL_CLASS}."
          exit 1
        fi
        CORPUS_TOP=$(( CORPUS_TOP - 1 ))
      done
      if [ "$CORPUS_TOP" -ne "$HEAD_DEC" ]; then
        echo "Newest traceable block is ${CORPUS_TOP}, $(( HEAD_DEC - CORPUS_TOP )) below the tip"
      fi
      while [ "$CALL_BLOCKS" -gt 1 ] && ! block_is_traceable $(( CORPUS_TOP - CALL_BLOCKS + 1 )); do
        CALL_BLOCKS=$(( CALL_BLOCKS / 2 ))
        echo "Retrying traceability probe with ${CALL_BLOCKS} source blocks"
      done
      echo "Snapshot traces blocks $(( CORPUS_TOP - CALL_BLOCKS + 1 ))..${CORPUS_TOP}"
      ;;
  esac
fi

# Stop the throwaway node and recover the snapshot before extraction. Call mode
# keeps it up: the trace classes read their blocks from it, and the exit trap
# stops it once the corpus is staged.
if [ "$EXECUTION_MODE" != "call" ]; then
  sudo systemctl stop "$RETH_SCOPE" 2>/dev/null || true
  sudo systemctl reset-failed "$RETH_SCOPE" 2>/dev/null || true
  kill "${TAIL_PID:-}" 2>/dev/null || true
  TAIL_PID=
  sudo schelk recover -y --kill || true
fi

# --- Stage the call corpus ---

if [ "$EXECUTION_MODE" = "call" ]; then
  CALL_METHODS="${BENCH_CALL_METHODS:-}"
  CORPUS_FILE="$OUTPUT_DIR/corpus.jsonl"

  read_corpus() {
    case "$1" in
      *.gz) gzip -dc "$1" ;;
      *) cat "$1" ;;
    esac
  }

  if [ "$CORPUS_SOURCE" = "static" ]; then
    TXGEN_ETHEREUM="$(which txgen-ethereum)"
    if [ "$CALL_BLOCKS" -le 0 ]; then
      echo "::error::BENCH_BLOCKS must be greater than 0"
      exit 1
    fi
    case "$CALL_CLASS" in
      call|tracecall)
        # Transactions of the blocks after the tip are exactly the calls a
        # client would have served against the tip state; only the archive
        # node has those blocks. When it is unreachable, the blocks just below
        # the tip on the snapshot stand in: their calls replay against the tip
        # state too, only with more reverts.
        CORPUS_FORMAT=calls
        if archive_rpc_reachable; then
          CORPUS_FROM=$(( HEAD_DEC + 1 ))
          CORPUS_TO=$(( HEAD_DEC + CALL_BLOCKS ))
          CORPUS_RPC="$BENCH_RPC_URL"
          CORPUS_BLOCKS_SOURCE=archive
        else
          echo "::warning::Archive node ${BENCH_RPC_URL} is unreachable; building the ${CALL_CLASS} corpus from snapshot blocks below the tip"
          CORPUS_FROM=$(( HEAD_DEC - CALL_BLOCKS + 1 ))
          CORPUS_TO="$HEAD_DEC"
          CORPUS_RPC="http://127.0.0.1:8545"
          CORPUS_BLOCKS_SOURCE=snapshot
          # These transactions already executed, so replay each against the
          # state before its block instead of the tip, where its preconditions
          # no longer hold.
          CORPUS_BLOCK_PARAM=parent
        fi
        ;;
      tracetx|traceblock)
        # These blocks are on the snapshot, so the throwaway node serves them
        # and the archive node is not needed. CORPUS_TOP is the newest block the
        # probe found traceable, which may sit below the header tip.
        CORPUS_FROM=$(( CORPUS_TOP - CALL_BLOCKS + 1 ))
        CORPUS_TO="$CORPUS_TOP"
        CORPUS_FORMAT=traces
        CORPUS_RPC="http://127.0.0.1:8545"
        CORPUS_BLOCKS_SOURCE=snapshot
        ;;
      *)
        echo "::error::Unknown call class: ${CALL_CLASS}"
        exit 1
        ;;
    esac
    CORPUS_METHOD_ARGS=()
    if [ -n "$CALL_METHODS" ]; then
      CORPUS_METHOD_ARGS+=(--methods "$CALL_METHODS")
    fi
    if [ -n "${BENCH_CALL_TOP_GAS:-}" ]; then
      CORPUS_METHOD_ARGS+=(--top-gas "$BENCH_CALL_TOP_GAS")
    fi
    if [ -n "${BENCH_CALL_TRACER:-}" ]; then
      IFS=',' read -r -a tracer_specs <<< "$BENCH_CALL_TRACER"
      for spec in "${tracer_specs[@]}"; do
        spec="${spec// /}"
        [ -z "$spec" ] && continue
        case "$spec" in
          js:*)
            tracer_name="${spec#js:}"
            case "$tracer_name" in
              */*|.*|"")
                echo "::error::Invalid tracer name: ${spec}"
                exit 1
                ;;
            esac
            tracer_file="${BENCH_CORPUS_DIR:-/reth-bench/corpora}/tracers/${tracer_name}.js"
            if [ ! -f "$tracer_file" ]; then
              echo "::error::JS tracer ${tracer_name} not found at ${tracer_file}"
              exit 1
            fi
            CORPUS_METHOD_ARGS+=(--tracer "js:${tracer_file}")
            ;;
          *)
            CORPUS_METHOD_ARGS+=(--tracer "$spec")
            ;;
        esac
      done
    fi
    if [ -n "${BENCH_CALL_TRACER_CONFIG:-}" ]; then
      CORPUS_METHOD_ARGS+=(--tracer-config "$BENCH_CALL_TRACER_CONFIG")
    fi
    if [ -n "${CORPUS_BLOCK_PARAM:-}" ]; then
      CORPUS_METHOD_ARGS+=(--block-param "$CORPUS_BLOCK_PARAM")
    fi
    if [ -n "${BENCH_CALL_TRACE_OPTIONS:-}" ]; then
      CORPUS_METHOD_ARGS+=(--trace-options "$BENCH_CALL_TRACE_OPTIONS")
    fi
    echo "Generating ${CALL_CLASS} corpus from blocks ${CORPUS_FROM}..${CORPUS_TO} (methods: ${CALL_METHODS:-default})"
    "$TXGEN_ETHEREUM" extract \
      --rpc "$CORPUS_RPC" \
      --from "$CORPUS_FROM" \
      --to "$CORPUS_TO" \
      --format "$CORPUS_FORMAT" \
      "${CORPUS_METHOD_ARGS[@]}" \
      -o "$CORPUS_FILE"
  else
    case "$CORPUS_SOURCE" in
      */*|.*)
        echo "::error::Invalid corpus name: ${CORPUS_SOURCE}"
        exit 1
        ;;
    esac
    CORPUS_DIR="${BENCH_CORPUS_DIR:-/reth-bench/corpora}"
    CORPUS_SRC=""
    for candidate in "$CORPUS_DIR/$CORPUS_SOURCE.jsonl.gz" "$CORPUS_DIR/$CORPUS_SOURCE.jsonl"; do
      if [ -f "$candidate" ]; then
        CORPUS_SRC="$candidate"
        break
      fi
    done
    if [ -z "$CORPUS_SRC" ]; then
      echo "::error::Corpus ${CORPUS_SOURCE} not found in ${CORPUS_DIR} (expected ${CORPUS_SOURCE}.jsonl.gz or ${CORPUS_SOURCE}.jsonl)"
      exit 1
    fi
    CORPUS_SHA256="$CORPUS_DIR/$CORPUS_SOURCE.sha256"
    if [ -f "$CORPUS_SHA256" ]; then
      EXPECTED_SHA="$(awk '{print $1; exit}' "$CORPUS_SHA256")"
      ACTUAL_SHA="$(sha256sum "$CORPUS_SRC" | awk '{print $1}')"
      if [ "$EXPECTED_SHA" != "$ACTUAL_SHA" ]; then
        echo "::error::Checksum mismatch for corpus ${CORPUS_SOURCE}"
        exit 1
      fi
      echo "Corpus ${CORPUS_SOURCE} checksum verified"
    fi
    case "$CORPUS_SRC" in
      *.gz) CORPUS_FILE="$OUTPUT_DIR/corpus.jsonl.gz" ;;
    esac
    cp "$CORPUS_SRC" "$CORPUS_FILE"
  fi

  CORPUS_RECORDS=$(read_corpus "$CORPUS_FILE" | wc -l | tr -d ' ')
  if [ "$CORPUS_RECORDS" -le 0 ]; then
    echo "::error::Corpus ${CORPUS_SOURCE} is empty"
    exit 1
  fi
  CORPUS_RECORDS_PER_METHOD=$(read_corpus "$CORPUS_FILE" | jq -r '.method' | sort | uniq -c \
    | jq -R -s 'split("\n")
        | map(select(length > 0) | capture("^\\s*(?<count>\\d+)\\s+(?<method>.+)$") | {(.method): (.count | tonumber)})
        | add // {}')
  jq -n \
    --arg source "$CORPUS_SOURCE" \
    --arg name "$CORPUS_SOURCE" \
    --arg class "$CALL_CLASS" \
    --arg methods "$CALL_METHODS" \
    --argjson records "$CORPUS_RECORDS" \
    --argjson records_per_method "$CORPUS_RECORDS_PER_METHOD" \
    --argjson tip "$HEAD_DEC" \
    --arg tip_hash "$HEAD_HASH" \
    --arg blocks_source "${CORPUS_BLOCKS_SOURCE:-}" \
    --arg block_param "${CORPUS_BLOCK_PARAM:-}" \
    --argjson corpus_from "${CORPUS_FROM:-null}" \
    --argjson corpus_to "${CORPUS_TO:-null}" \
    --argjson top_gas "${BENCH_CALL_TOP_GAS:-null}" \
    --arg tracer "${BENCH_CALL_TRACER:-}" \
    --arg namespace "${BENCH_CALL_NAMESPACE:-}" \
    --argjson tracer_config "${BENCH_CALL_TRACER_CONFIG:-null}" \
    --argjson trace_options "${BENCH_CALL_TRACE_OPTIONS:-null}" \
    '{
      source: (if $source == "static" then "static" else "custom" end),
      name: $name,
      class: $class,
      methods: (if $methods == "" then [] else ($methods | split(",")) end),
      records: $records,
      records_per_method: $records_per_method,
      tip: $tip,
      tip_hash: $tip_hash,
      blocks_source: (if $blocks_source == "" then null else $blocks_source end),
      block_param: (if $block_param == "" then null else $block_param end),
      corpus_from: $corpus_from,
      corpus_to: $corpus_to,
      top_gas: $top_gas,
      tracer: (if $tracer == "" then null else ($tracer | split(",")) end),
      namespace: (if $namespace == "" then null else $namespace end),
      tracer_config: $tracer_config,
      trace_options: $trace_options,
    }' > "$OUTPUT_DIR/corpus.meta.json"

  echo "Corpus staged: ${CORPUS_RECORDS} records in ${CORPUS_FILE}"
  exit 0
fi

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
EXTRACT_FROM=$(( HEAD_DEC + 1 ))
TXGEN_EXTRACT_ARGS=()
if [ "$INCLUDE_BAL" = "true" ]; then
  TXGEN_EXTRACT_ARGS+=(--bal)
fi

if [ "$EXECUTION_MODE" = "rpc" ]; then
  ALL_BLOCKS="$OUTPUT_DIR/all-transactions.ndjson"
  WARMUP_FILE="$OUTPUT_DIR/warmup-transactions.ndjson"
  BENCHMARK_FILE="$OUTPUT_DIR/benchmark-transactions.ndjson"
  if [ "$BIG_BLOCKS" = "true" ] || [ "$INCLUDE_BAL" = "true" ]; then
    echo "::error::RPC mode does not support big blocks or BAL"
    exit 1
  fi

  EXTRACT_TO=$(( HEAD_DEC + TOTAL ))
  echo "Extracting transactions from blocks ${EXTRACT_FROM}..${EXTRACT_TO} for RPC benchmark (${WARMUP} warmup blocks, ${BLOCKS} measured blocks)"
  if [ "$WARMUP" -gt 0 ] 2>/dev/null; then
    "$TXGEN_ETHEREUM" extract \
      --rpc "$BENCH_RPC_URL" \
      --from "$EXTRACT_FROM" \
      --to "$(( HEAD_DEC + WARMUP ))" \
      --format transactions \
      -o "$WARMUP_FILE"
  else
    : > "$WARMUP_FILE"
  fi
  "$TXGEN_ETHEREUM" extract \
    --rpc "$BENCH_RPC_URL" \
    --from "$(( HEAD_DEC + WARMUP + 1 ))" \
    --to "$EXTRACT_TO" \
    --format transactions \
    -o "$BENCHMARK_FILE"
  cat "$WARMUP_FILE" "$BENCHMARK_FILE" > "$ALL_BLOCKS"
elif [ "$BIG_BLOCKS" = "true" ]; then
  ALL_BLOCKS="$OUTPUT_DIR/all-big-blocks.ndjson"
  WARMUP_FILE="$OUTPUT_DIR/warmup-big-blocks.ndjson"
  BENCHMARK_FILE="$OUTPUT_DIR/measured-big-blocks.ndjson"
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

# Block output has one line per source block. Transaction output is extracted
# separately above because it has one line per transaction.
if [ "$EXECUTION_MODE" = "engine" ]; then
  if [ "$WARMUP" -gt 0 ] 2>/dev/null; then
    head -n "$WARMUP" "$ALL_BLOCKS" > "$WARMUP_FILE"
  else
    : > "$WARMUP_FILE"
  fi
  awk -v warmup="$WARMUP" 'NR > warmup { print }' "$ALL_BLOCKS" > "$BENCHMARK_FILE"
fi

if [ "$INCLUDE_BAL" = "true" ] && [ "$BAL_MODE" != "true" ]; then
  echo "Writing no-BAL payload variants for selective BAL mode (${BAL_MODE})"
  for file in "$ALL_BLOCKS" "$WARMUP_FILE" "$BENCHMARK_FILE"; do
    jq -c 'del(.bal, .merged_block_access_list)' "$file" > "${file%.ndjson}-no-bal.ndjson"
  done
fi

echo "Extraction complete: $(wc -l < "$ALL_BLOCKS") ${EXECUTION_MODE} records in ${OUTPUT_DIR}"
