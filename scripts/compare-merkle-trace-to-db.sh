#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

exec cargo run --profile profiling -p example-db-access --bin compare_merkle_trace_to_db -- "$@"
