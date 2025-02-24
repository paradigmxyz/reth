#!/usr/bin/env bash
set -eo pipefail

# TODO: Benchmarks run WAY too slow due to excessive amount of iterations.

cmd=(cargo codspeed build --profile profiling)
crates=(
    -p reth-primitives
    -p reth-trie
    -p reth-trie-common
    -p reth-trie-sparse
)

"${cmd[@]}" --features test-utils "${crates[@]}"
