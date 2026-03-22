#!/usr/bin/env bash
set -uo pipefail

crates_to_check=(
    reth-codecs-derive
    reth-primitives
    reth-primitives-traits
    reth-network-peers
    reth-trie-common
    reth-trie-sparse
    reth-chainspec
    reth-consensus
    reth-consensus-common
    reth-prune-types
    reth-static-file-types
    reth-storage-errors
    reth-execution-errors
    reth-errors
    reth-execution-types
    reth-db-models
    reth-evm
    reth-revm
    reth-storage-api

    ## ethereum
    reth-evm-ethereum
    reth-ethereum-forks
    reth-ethereum-primitives
    reth-ethereum-consensus
)

any_failed=0
tmpdir=$(mktemp -d 2>/dev/null || mktemp -d -t reth-check)
trap 'rm -rf -- "$tmpdir"' EXIT INT TERM

for crate in "${crates_to_check[@]}"; do
  outfile="$tmpdir/$crate.log"
  if cargo +stable build -p "$crate" --target riscv32imac-unknown-none-elf --no-default-features --color never >"$outfile" 2>&1; then
    echo "✅ $crate"
  else
    echo "❌ $crate"
    sed 's/^/   /' "$outfile"
    echo ""
    any_failed=1
  fi
done

exit $any_failed
