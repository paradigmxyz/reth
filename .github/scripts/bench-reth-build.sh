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
#   baseline: <source-dir>/target/profiling/reth (or reth-bb if BENCH_BIG_BLOCKS=true)
#   feature:  <source-dir>/target/profiling/reth (or reth-bb), reth-bench installed to cargo bin
#
# Required: mc (MinIO client) with a configured alias
# Optional env: BENCH_BIG_BLOCKS (true/false) — build reth-bb instead of reth
set -euo pipefail

MC="mc"
MODE="$1"
SOURCE_DIR="$2"
COMMIT="$3"

BIG_BLOCKS="${BENCH_BIG_BLOCKS:-false}"
# The node binary to build: reth-bb for big blocks, reth otherwise
if [ "$BIG_BLOCKS" = "true" ]; then
  NODE_BIN="reth-bb"
  NODE_PKG="-p reth-bb"
else
  NODE_BIN="reth"
  NODE_PKG="--bin reth"
fi

# Tracy support: when BENCH_TRACY is "on" or "full", add Tracy cargo features
# and frame pointers for accurate stack traces.
EXTRA_FEATURES=""
EXTRA_RUSTFLAGS=""
if [ "${BENCH_TRACY:-off}" != "off" ]; then
  EXTRA_FEATURES="tracy,tracy-client/ondemand"
  EXTRA_RUSTFLAGS=" -C force-frame-pointers=yes"
fi

# Cache suffix: hash of features+rustflags so different build configs get separate cache entries
if [ -n "$EXTRA_FEATURES" ] || [ -n "$EXTRA_RUSTFLAGS" ]; then
  BUILD_SUFFIX="-$(echo "${EXTRA_FEATURES}${EXTRA_RUSTFLAGS}" | sha256sum | cut -c1-12)"
else
  BUILD_SUFFIX=""
fi

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
    BUCKET="minio/reth-binaries/${COMMIT}${BUILD_SUFFIX}"
    mkdir -p "${SOURCE_DIR}/target/profiling"

    CACHE_VALID=false
    if $MC stat "${BUCKET}/${NODE_BIN}" &>/dev/null; then
      echo "Cache hit for baseline (${COMMIT}), downloading ${NODE_BIN}..."
      $MC cp "${BUCKET}/${NODE_BIN}" "${SOURCE_DIR}/target/profiling/${NODE_BIN}"
      chmod +x "${SOURCE_DIR}/target/profiling/${NODE_BIN}"
      if verify_binary "${SOURCE_DIR}/target/profiling/${NODE_BIN}" "${COMMIT}"; then
        CACHE_VALID=true
      else
        echo "Cached baseline binary is stale, rebuilding..."
      fi
    fi
    if [ "$CACHE_VALID" = false ]; then
      echo "Building baseline ${NODE_BIN} (${COMMIT}) from source..."
      cd "${SOURCE_DIR}"
      FEATURES_ARG=""
      WORKSPACE_ARG=""
      if [ -n "$EXTRA_FEATURES" ]; then
        # --workspace is needed for cross-package feature syntax (tracy-client/ondemand)
        FEATURES_ARG="--features ${EXTRA_FEATURES}"
        WORKSPACE_ARG="--workspace"
      fi
      # shellcheck disable=SC2086
      RUSTFLAGS="-C target-cpu=native${EXTRA_RUSTFLAGS}" \
        cargo build --profile profiling $NODE_PKG $WORKSPACE_ARG $FEATURES_ARG
      $MC cp "target/profiling/${NODE_BIN}" "${BUCKET}/${NODE_BIN}"
    fi
    ;;

  feature|branch)
    BRANCH_SHA="${4:-$COMMIT}"
    BUCKET="minio/reth-binaries/${BRANCH_SHA}${BUILD_SUFFIX}"

    CACHE_VALID=false
    if $MC stat "${BUCKET}/${NODE_BIN}" &>/dev/null && $MC stat "${BUCKET}/reth-bench" &>/dev/null; then
      echo "Cache hit for ${BRANCH_SHA}, downloading binaries..."
      mkdir -p "${SOURCE_DIR}/target/profiling"
      $MC cp "${BUCKET}/${NODE_BIN}" "${SOURCE_DIR}/target/profiling/${NODE_BIN}"
      $MC cp "${BUCKET}/reth-bench" /home/ubuntu/.cargo/bin/reth-bench
      chmod +x "${SOURCE_DIR}/target/profiling/${NODE_BIN}" /home/ubuntu/.cargo/bin/reth-bench
      if verify_binary "${SOURCE_DIR}/target/profiling/${NODE_BIN}" "${COMMIT}"; then
        CACHE_VALID=true
      else
        echo "Cached feature binary is stale, rebuilding..."
      fi
    fi
    if [ "$CACHE_VALID" = false ]; then
      echo "Building feature ${NODE_BIN} (${COMMIT}) from source..."
      cd "${SOURCE_DIR}"
      rustup show active-toolchain || rustup default stable
      if [ -n "$EXTRA_FEATURES" ]; then
        # Can't use `make profiling` when adding features; build explicitly
        # --workspace is needed for cross-package feature syntax (tracy-client/ondemand)
        RUSTFLAGS="-C target-cpu=native${EXTRA_RUSTFLAGS}" \
          cargo build --profile profiling --workspace $NODE_PKG --features "${EXTRA_FEATURES}"
      else
        # shellcheck disable=SC2086
        RUSTFLAGS="-C target-cpu=native${EXTRA_RUSTFLAGS}" \
          cargo build --profile profiling $NODE_PKG
      fi
      make install-reth-bench
      $MC cp "target/profiling/${NODE_BIN}" "${BUCKET}/${NODE_BIN}"
      $MC cp "$(which reth-bench)" "${BUCKET}/reth-bench"
    fi
    ;;

  *)
    echo "Usage: $0 <baseline|feature> <source-dir> <commit> [branch-sha]"
    exit 1
    ;;
esac
