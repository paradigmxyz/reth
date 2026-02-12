#!/usr/bin/env bash
set -uo pipefail

readarray -t crates < <(
  cargo metadata --format-version=1 --no-deps | jq -r '.packages[].name' | grep '^reth' | sort
)

# shellcheck disable=SC2034
exclude_crates=(
  # The following require investigation if they can be fixed
  reth-basic-payload-builder
  reth-bench
  reth-bench-compare
  reth-cli
  reth-cli-commands
  reth-cli-runner
  reth-consensus-debug-client
  reth-db-common
  reth-discv4
  reth-discv5
  reth-dns-discovery
  reth-downloaders
  reth-e2e-test-utils
  reth-engine-service
  reth-engine-tree
  reth-engine-util
  reth-eth-wire
  reth-ethereum-cli
  reth-ethereum-payload-builder
  reth-etl
  reth-exex
  reth-exex-test-utils
  reth-ipc
  reth-net-nat
  reth-network
  reth-node-api
  reth-node-builder
  reth-node-core
  reth-node-ethereum
  reth-node-events
  reth-node-metrics
  reth-rpc
  reth-rpc-api
  reth-rpc-api-testing-util
  reth-rpc-builder
  reth-rpc-convert
  reth-rpc-e2e-tests
  reth-rpc-engine-api
  reth-rpc-eth-api
  reth-rpc-eth-types
  reth-rpc-layer
  reth-stages
  reth-engine-local
  reth-ress-protocol
  reth-ress-provider
  # The following are not supposed to be working
  reth # all of the crates below
  reth-storage-rpc-provider
  reth-invalid-block-hooks # reth-provider
  reth-libmdbx # mdbx
  reth-mdbx-sys # mdbx
  reth-payload-builder # reth-metrics
  reth-provider # tokio
  reth-prune # tokio
  reth-prune-static-files # reth-provider
  reth-tasks # tokio rt-multi-thread
  reth-stages-api # reth-provider, reth-prune
  reth-static-file # tokio
  reth-transaction-pool # c-kzg
  reth-payload-util # reth-transaction-pool
  reth-trie-parallel # tokio
  reth-trie-sparse-parallel # rayon
  reth-testing-utils
  reth-era-downloader # tokio
  reth-era-utils # tokio
  reth-tracing-otlp
  reth-node-ethstats
)

any_failed=0
tmpdir=$(mktemp -d 2>/dev/null || mktemp -d -t reth-check)
trap 'rm -rf -- "$tmpdir"' EXIT INT TERM

contains() {
  local array="$1[@]"
  local seeking="$2"
  local element
  for element in "${!array}"; do
    [[ "$element" == "$seeking" ]] && return 0
  done
  return 1
}

for crate in "${crates[@]}"; do
  if contains exclude_crates "$crate"; then
    echo "⏭️ $crate"
    continue
  fi

  outfile="$tmpdir/$crate.log"
  if cargo +stable build -p "$crate" --target wasm32-wasip1 --no-default-features --color never >"$outfile" 2>&1; then
    echo "✅ $crate"
  else
    echo "❌ $crate"
    sed 's/^/   /' "$outfile"
    echo ""
    any_failed=1
  fi
done

exit $any_failed
