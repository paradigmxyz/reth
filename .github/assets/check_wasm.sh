#!/usr/bin/env bash
set -eo pipefail

wasm_packages=(
  reth-codecs
  reth-consensus
  reth-db
  reth-db-api
  reth-ethereum-forks
  reth-evm
  reth-evm-ethereum
  reth-network-peers
  reth-primitives
  reth-primitives-traits
  reth-revm
)

for package in "${wasm_packages[@]}"; do
  cmd="cargo +stable build -p $package --target wasm32-wasi --no-default-features --ignore-rust-version --features std"

  if [ -n "$CI" ]; then
    echo "::group::$cmd1 || $cmd2"
  else
    printf "\n%s:\n  %s\n" "$package" "$cmd"
  fi

  $cmd

  if [ -n "$CI" ]; then
    echo "::endgroup::"
  fi
done