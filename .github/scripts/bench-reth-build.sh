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
# Required: mc (MinIO client) with a configured alias
set -euo pipefail

MC="mc"
MODE="$1"
SOURCE_DIR="$2"
COMMIT="$3"

# Verify a cached reth binary was built from the expected commit.
# `reth --version` outputs "Commit SHA: <full-sha>" on its own line.
verify_binary() {
  local binary="$1" expected_commit="$2"
  local version binary_sha
  version=$("$binary" --version 2>/dev/null) || return 1
  binary_sha=$(echo "$version" | sed -n 's/^Commit SHA: *//p')
  if [ -z "$binary_sha" ]; then
    echo "Warning: could not extract commit SHA from version output"
    return 1
  fi
  if [ "$binary_sha" = "$expected_commit" ]; then
    return 0
  fi
  echo "Cache mismatch: binary built from ${binary_sha} but expected ${expected_commit}"
  return 1
}

case "$MODE" in
  baseline|main)
    BUCKET="minio/reth-binaries/${COMMIT}"
    mkdir -p "${SOURCE_DIR}/target/profiling"

    CACHE_VALID=false
    if $MC stat "${BUCKET}/reth" &>/dev/null; then
      echo "Cache hit for baseline (${COMMIT}), downloading binary..."
      $MC cp "${BUCKET}/reth" "${SOURCE_DIR}/target/profiling/reth"
      chmod +x "${SOURCE_DIR}/target/profiling/reth"
      if verify_binary "${SOURCE_DIR}/target/profiling/reth" "${COMMIT}"; then
        CACHE_VALID=true
      else
        echo "Cached baseline binary is stale, rebuilding..."
      fi
    fi
    if [ "$CACHE_VALID" = false ]; then
      echo "Building baseline (${COMMIT}) from source..."
      cd "${SOURCE_DIR}"
      cargo build --profile profiling --bin reth
      $MC cp target/profiling/reth "${BUCKET}/reth"
    fi
    ;;

  feature|branch)
    BRANCH_SHA="${4:-$COMMIT}"
    BUCKET="minio/reth-binaries/${BRANCH_SHA}"

    CACHE_VALID=false
    if $MC stat "${BUCKET}/reth" &>/dev/null && $MC stat "${BUCKET}/reth-bench" &>/dev/null; then
      echo "Cache hit for ${BRANCH_SHA}, downloading binaries..."
      mkdir -p "${SOURCE_DIR}/target/profiling"
      $MC cp "${BUCKET}/reth" "${SOURCE_DIR}/target/profiling/reth"
      $MC cp "${BUCKET}/reth-bench" /home/ubuntu/.cargo/bin/reth-bench
      chmod +x "${SOURCE_DIR}/target/profiling/reth" /home/ubuntu/.cargo/bin/reth-bench
      if verify_binary "${SOURCE_DIR}/target/profiling/reth" "${COMMIT}"; then
        CACHE_VALID=true
      else
        echo "Cached feature binary is stale, rebuilding..."
      fi
    fi
    if [ "$CACHE_VALID" = false ]; then
      echo "Building feature (${COMMIT}) from source..."
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
