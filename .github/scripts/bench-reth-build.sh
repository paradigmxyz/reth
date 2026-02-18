#!/usr/bin/env bash
#
# Builds (or fetches from cache) reth binaries for benchmarking.
#
# Usage: bench-reth-build.sh <baseline|feature> <source-dir> <commit> [branch-sha]
#
#   baseline — build/fetch the baseline binary at <commit> (merge-base)
#              source-dir must be checked out at <commit>
#   feature  — build/fetch the candidate binary + reth-bench at <commit>
#              source-dir must be checked out at <commit>
#              optional branch-sha is the PR head commit for cache key
#
# Outputs:
#   baseline: <source-dir>/target/profiling/reth
#   feature:  <source-dir>/target/profiling/reth, reth-bench installed to cargo bin
#
# Required: mc (MinIO client) configured at /home/ubuntu/.mc
set -euo pipefail

MC="mc --config-dir /home/ubuntu/.mc"
MODE="$1"
SOURCE_DIR="$2"
COMMIT="$3"

case "$MODE" in
  baseline|main)
    BUCKET="minio/reth-binaries/${COMMIT}"
    mkdir -p "${SOURCE_DIR}/target/profiling"

    if $MC stat "${BUCKET}/reth" &>/dev/null; then
      echo "Cache hit for baseline (${COMMIT}), downloading binary..."
      $MC cp "${BUCKET}/reth" "${SOURCE_DIR}/target/profiling/reth"
      chmod +x "${SOURCE_DIR}/target/profiling/reth"
    else
      echo "Cache miss for baseline (${COMMIT}), building from source..."
      cd "${SOURCE_DIR}"
      cargo build --profile profiling --bin reth
      $MC cp target/profiling/reth "${BUCKET}/reth"
    fi
    ;;

  feature|branch)
    BRANCH_SHA="${4:-$COMMIT}"
    BUCKET="minio/reth-binaries/${BRANCH_SHA}"

    if $MC stat "${BUCKET}/reth" &>/dev/null && $MC stat "${BUCKET}/reth-bench" &>/dev/null; then
      echo "Cache hit for ${BRANCH_SHA}, downloading binaries..."
      mkdir -p "${SOURCE_DIR}/target/profiling"
      $MC cp "${BUCKET}/reth" "${SOURCE_DIR}/target/profiling/reth"
      $MC cp "${BUCKET}/reth-bench" /home/ubuntu/.cargo/bin/reth-bench
      chmod +x "${SOURCE_DIR}/target/profiling/reth" /home/ubuntu/.cargo/bin/reth-bench
    else
      echo "Cache miss for ${BRANCH_SHA}, building from source..."
      cd "${SOURCE_DIR}"
      rustup show active-toolchain || rustup default stable
      make profiling
      make install-reth-bench
      $MC cp target/profiling/reth "${BUCKET}/reth"
      $MC cp "$(which reth-bench)" "${BUCKET}/reth-bench"
    fi
    ;;

  *)
    echo "Usage: $0 <baseline|feature> <source-dir> <commit> [branch-sha]"
    exit 1
    ;;
esac
