#!/usr/bin/env bash
set -euo pipefail

usage() {
    echo "Usage: $0 [-n NUM_BLOCKS] [-b BLOCK] <remote_rpc_url>"
    echo ""
    echo "Fetches debug_executionWitness from both localhost:8545 and the"
    echo "given remote RPC, then compares them."
    echo ""
    echo "Options:"
    echo "  -n NUM   Compare the last NUM blocks (default: 20)"
    echo "  -b BLOCK Compare only the given block number"
    exit 1
}

NUM_BLOCKS=20
SINGLE_BLOCK=""

while getopts "n:b:h" opt; do
    case "$opt" in
        n) NUM_BLOCKS="$OPTARG" ;;
        b) SINGLE_BLOCK="$OPTARG" ;;
        *) usage ;;
    esac
done
shift $((OPTIND - 1))

if [[ $# -lt 1 ]]; then
    echo "Error: remote_rpc_url is required" >&2
    usage
fi
REMOTE_RPC="$1"
LOCAL_RPC="http://127.0.0.1:8545"

log()  { echo "[$(date '+%H:%M:%S')] $*" >&2; }

if [[ -n "$SINGLE_BLOCK" ]]; then
    start=$SINGLE_BLOCK
    latest=$SINGLE_BLOCK
    log "Comparing block $SINGLE_BLOCK"
else
    # Get latest block number from local node
    if ! latest=$(cast bn --rpc-url "$LOCAL_RPC" --rpc-timeout 600 2>&1); then
        log "FATAL: failed to get block number from local RPC: $latest"
        exit 1
    fi

    start=$((latest - NUM_BLOCKS + 1))
    if (( start < 0 )); then
        start=0
    fi

    log "Comparing blocks $start..$latest ($NUM_BLOCKS blocks)"
fi

errors=0

for (( block = start; block <= latest; block++ )); do
    block_hex=$(printf '"0x%x"' "$block")
    log "Checking block $block ($block_hex)"

    # Fetch witness from both RPCs
    local_witness=$(cast rpc debug_executionWitness "$block_hex" --rpc-url "$LOCAL_RPC" --rpc-timeout 600 2>&1) || {
        log "WARN: failed to get witness from local RPC for block $block: $local_witness"
        ((errors++)) || true
        continue
    }

    remote_witness=$(cast rpc debug_executionWitness "$block_hex" --rpc-url "$REMOTE_RPC" --rpc-timeout 600 2>&1) || {
        log "WARN: failed to get witness from remote RPC for block $block: $remote_witness"
        ((errors++)) || true
        continue
    }

    # Normalize: sort all arrays of objects by a stable key so ordering doesn't cause false diffs
    normalize='walk(if type == "array" then sort_by(if type == "object" then (keys | join(",")) + ":" + (to_entries | map(.value | tostring) | join(",")) else tostring end) else . end) | . as $root | $root'
    local_file=$(mktemp)
    remote_file=$(mktemp)
    trap "rm -f '$local_file' '$remote_file'" EXIT
    echo "$local_witness"  | jq -S "$normalize" > "$local_file"
    echo "$remote_witness" | jq -S "$normalize" > "$remote_file"

    # Compare: for "state", local may contain extra nodes (superset OK).
    # For "codes", "keys", "headers", require exact set equality.
    has_error=false

    # Check exact-match fields (as sorted sets)
    for field in codes keys headers; do
        local_set=$(jq -r --arg f "$field" '.[$f] // [] | sort | .[]' "$local_file")
        remote_set=$(jq -r --arg f "$field" '.[$f] // [] | sort | .[]' "$remote_file")
        if [[ "$local_set" != "$remote_set" ]]; then
            log "ERROR: block $block field '$field' differs"
            diff <(echo "$remote_set") <(echo "$local_set") | head -30 || true
            has_error=true
        fi
    done

    # Check state: every remote node must be present in local (extras OK)
    missing=$(jq -r -n \
        --slurpfile l "$local_file" \
        --slurpfile r "$remote_file" \
        '($l[0].state // [] | map({(.):true}) | add // {}) as $local_set |
         [$r[0].state // [] | .[] | select($local_set[.] | not)] |
         if length == 0 then empty else .[] end')
    if [[ -n "$missing" ]]; then
        n_missing=$(echo "$missing" | wc -l)
        log "ERROR: block $block state has $n_missing missing node(s) (present in remote, absent in local):"
        echo "$missing" | head -20
        has_error=true
    fi

    extra=$(jq -r -n \
        --slurpfile l "$local_file" \
        --slurpfile r "$remote_file" \
        '($r[0].state // [] | map({(.):true}) | add // {}) as $remote_set |
         [$l[0].state // [] | .[] | select($remote_set[.] | not)] |
         if length == 0 then empty else .[] end')
    n_extra=0
    if [[ -n "$extra" ]]; then
        n_extra=$(echo "$extra" | wc -l)
    fi

    if ! $has_error; then
        if [[ $n_extra -gt 0 ]]; then
            log "OK: block $block witnesses match ($n_extra extra state node(s) in local)"
        else
            log "OK: block $block witnesses match exactly"
        fi
    else
        cp "$local_file" "witness-local-${block}.json"
        cp "$remote_file" "witness-remote-${block}.json"
        log "Wrote witness-local-${block}.json and witness-remote-${block}.json"
        if [[ $n_extra -gt 0 ]]; then
            log "  (local also has $n_extra extra state node(s))"
        fi
        ((errors++)) || true
        log "---"
    fi

    rm -f "$local_file" "$remote_file"
done

total=$((latest - start + 1))
if (( errors > 0 )); then
    log "DONE: $errors block(s) had errors out of $total"
    exit 1
else
    log "DONE: all $total block(s) matched"
fi
