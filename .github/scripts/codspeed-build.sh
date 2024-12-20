#!/usr/bin/env bash
set -eo pipefail

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
)

"${cmd[@]}" --features test-utils --workspace "${excludes[@]}"
"${cmd[@]}" -p reth-transaction-pool --features test-utils,arbitrary
