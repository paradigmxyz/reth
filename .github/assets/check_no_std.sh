#!/usr/bin/env bash
set -eo pipefail

# TODO
no_std_packages=(
# The following were confirmed not working in the past, but could be enabled if issues have been resolved
#   reth-db
#   reth-primitives
#   reth-revm
#   reth-evm
#   reth-evm-ethereum
#   reth-consensus
# the following are confirmed working
    reth-errors
    reth-ethereum-forks
    reth-network-peers
    reth-primitives-traits
    reth-codecs
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
