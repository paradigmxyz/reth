#!/usr/bin/env bash
set -euo pipefail

sudo rm -rf ../reth-baseline/target ../reth-feature/target "$BENCH_WORK_DIR" 2>/dev/null || true
