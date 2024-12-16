#!/usr/bin/env bash
set -eo pipefail

cmd=(cargo codspeed build --profile profiling)
"${cmd[@]}" --features test-utils \
    --workspace \
    --exclude reth-libmdbx \
    --exclude reth-transaction-pool

# TODO: broken
# "${cmd[@]}" -p reth-transaction-pool --features test-utils,arbitrary
