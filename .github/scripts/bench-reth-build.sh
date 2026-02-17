#!/usr/bin/env bash
#
# Builds (or fetches from cache) reth binaries for benchmarking.
#
# Usage: bench-reth-build.sh <main|branch> <commit> [branch-sha]
#
#   main   — build/fetch the baseline binary at <commit> (merge-base)
#   branch — build/fetch the candidate binary + reth-bench at <commit>
#            optional branch-sha is the PR head commit for cache key
#
# Outputs:
#   main:   target/profiling-baseline/reth
#   branch: target/profiling/reth, reth-bench installed to cargo bin
#
# Required: mc (MinIO client) configured at /home/ubuntu/.mc
set -euo pipefail

MC="mc --config-dir /home/ubuntu/.mc"
MODE="$1"
COMMIT="$2"

case "$MODE" in
  main)
    BUCKET="minio/reth-binaries/${COMMIT}"
    mkdir -p target/profiling-baseline

    if $MC stat "${BUCKET}/reth" &>/dev/null; then
      echo "Cache hit for main (${COMMIT}), downloading binary..."
      $MC cp "${BUCKET}/reth" target/profiling-baseline/reth
      chmod +x target/profiling-baseline/reth
    else
      echo "Cache miss for main (${COMMIT}), building from source..."
      CURRENT_REF=$(git rev-parse HEAD)
      git checkout "${COMMIT}"
      cargo build --profile profiling --bin reth
      cp target/profiling/reth target/profiling-baseline/reth
      $MC cp target/profiling-baseline/reth "${BUCKET}/reth"
      git checkout "${CURRENT_REF}"
    fi
    ;;

  branch)
    BRANCH_SHA="${3:-$COMMIT}"
    BUCKET="minio/reth-binaries/${BRANCH_SHA}"

    if $MC stat "${BUCKET}/reth" &>/dev/null && $MC stat "${BUCKET}/reth-bench" &>/dev/null; then
      echo "Cache hit for ${BRANCH_SHA}, downloading binaries..."
      mkdir -p target/profiling
      $MC cp "${BUCKET}/reth" target/profiling/reth
      $MC cp "${BUCKET}/reth-bench" /home/ubuntu/.cargo/bin/reth-bench
      chmod +x target/profiling/reth /home/ubuntu/.cargo/bin/reth-bench
    else
      echo "Cache miss for ${BRANCH_SHA}, building from source..."
      rustup show active-toolchain || rustup default stable
      make profiling
      make install-reth-bench
      $MC cp target/profiling/reth "${BUCKET}/reth"
      $MC cp "$(which reth-bench)" "${BUCKET}/reth-bench"
    fi
    ;;

  *)
    echo "Usage: $0 <main|branch> <commit> [branch-sha]"
    exit 1
    ;;
esac
