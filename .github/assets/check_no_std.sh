#!/usr/bin/env bash
set -eo pipefail

# TODO
no_std_packages=(
#   reth-db
#   reth-primitives
#   reth-codecs
#   reth-consensus
# the following are confirmed working
    reth-errors
    reth-ethereum-forks
    reth-evm
#   reth-evm-ethereum
    reth-network-peers
#   reth-primitives-traits
    reth-revm
)

for package in "${no_std_packages[@]}"; do
  cmd="cargo +stable build -p $package --target wasm32-wasip1 --no-default-features"

  if [ -n "$CI" ]; then
    echo "::group::$cmd"
  else
    printf "\n%s:\n  %s\n" "$package" "$cmd"
  fi

  $cmd

  if [ -n "$CI" ]; then
    echo "::endgroup::"
  fi
done
