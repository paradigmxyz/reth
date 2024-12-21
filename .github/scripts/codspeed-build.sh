#!/usr/bin/env bash
set -eo pipefail

# TODO: Benchmarks run WAY too slow due to excessive amount of iterations.

cmd=(cargo codspeed build --profile profiling)
excludes=(
    # Unnecessary
    --exclude reth-libmdbx
    # Build is too slow
    --exclude reth-network
    # Built separately 
    --exclude reth-transaction-pool
    # TODO: some benchmarks panic: https://github.com/paradigmxyz/reth/actions/runs/12307046814/job/34349955788
    --exclude reth-db
    --exclude reth-trie-parallel
    --exclude reth-engine-tree
)

"${cmd[@]}" --features test-utils --workspace "${excludes[@]}"

# TODO: Slow benchmarks due to too many iterations
## "${cmd[@]}" -p reth-transaction-pool --features test-utils,arbitrary
