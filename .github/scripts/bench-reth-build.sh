#!/usr/bin/env bash
#
# Builds (or fetches from cache) reth binaries for benchmarking.
#
# Usage: bench-reth-build.sh <baseline|feature> <source-dir> <commit> [cache-sha]
#
#   baseline — build/fetch the baseline binary at <commit> (merge-base)
#              source-dir must be checked out at <commit>
#   feature  — build/fetch the candidate binary + reth-bench at <commit>
#              source-dir must be checked out at <commit>
#              optional cache-sha is the PR head commit for cache key
#
# Outputs:
#   baseline: <source-dir>/target/profiling/reth (or reth-bb if BENCH_BIG_BLOCKS=true)
#   feature:  <source-dir>/target/profiling/reth (or reth-bb), reth-bench installed to cargo bin
#
# Optional env:
#   BENCH_BIG_BLOCKS (true/false) — build reth-bb instead of reth
#   BENCH_BINARY_CACHE_ROOT       — MinIO cache root (default: minio/reth-binaries)
set -euxo pipefail

MC="${MC:-mc}"
MODE="$1"
SOURCE_DIR="$2"
COMMIT="$3"
CACHE_ROOT="${BENCH_BINARY_CACHE_ROOT:-minio/reth-binaries}"
RETH_BENCH_BIN="${CARGO_HOME:-$HOME/.cargo}/bin/reth-bench"

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

hash_build_config() {
  if command -v sha256sum &>/dev/null; then
    sha256sum | cut -c1-12
  else
    shasum -a 256 | cut -c1-12
  fi
}

# Cache suffix: hash of features+rustflags so different build configs get separate cache entries.
if [ -n "$EXTRA_FEATURES" ] || [ -n "$EXTRA_RUSTFLAGS" ]; then
  BUILD_SUFFIX="-$(printf '%s' "${EXTRA_FEATURES}${EXTRA_RUSTFLAGS}" | hash_build_config)"
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

upload_cache_artifact() {
  local source="$1" destination="$2"
  if ! $MC cp "$source" "$destination"; then
    echo "::warning::Failed to upload binary cache artifact to ${destination}; continuing without remote cache write"
  fi
}

# Build the requested node binary with the benchmark profile.
build_node_binary() {
  local features_arg=""
  local workspace_arg=""

  cd "$SOURCE_DIR"
  if [ -n "$EXTRA_FEATURES" ]; then
    # --workspace is needed for cross-package feature syntax (tracy-client/ondemand)
    features_arg="--features ${EXTRA_FEATURES}"
    workspace_arg="--workspace"
  fi

  # shellcheck disable=SC2086
  RUSTFLAGS="-C target-cpu=native${EXTRA_RUSTFLAGS}" \
    cargo build --locked --profile profiling $NODE_PKG $workspace_arg $features_arg
}

case "$MODE" in
  baseline|main)
    BUCKET="${CACHE_ROOT}/${COMMIT}${BUILD_SUFFIX}"
    mkdir -p "${SOURCE_DIR}/target/profiling"

    CACHE_VALID=false
    if $MC stat --no-list "${BUCKET}/${NODE_BIN}" &>/dev/null; then
      echo "Cache hit for baseline (${COMMIT}), downloading ${NODE_BIN}..."
      if $MC cp "${BUCKET}/${NODE_BIN}" "${SOURCE_DIR}/target/profiling/${NODE_BIN}" && \
         chmod +x "${SOURCE_DIR}/target/profiling/${NODE_BIN}" && \
         verify_binary "${SOURCE_DIR}/target/profiling/${NODE_BIN}" "${COMMIT}"; then
        CACHE_VALID=true
      else
        echo "Cached baseline binary is stale or download failed, rebuilding..."
      fi
    fi
    if [ "$CACHE_VALID" = false ]; then
      echo "Building baseline ${NODE_BIN} (${COMMIT}) from source..."
      build_node_binary
      upload_cache_artifact "${SOURCE_DIR}/target/profiling/${NODE_BIN}" "${BUCKET}/${NODE_BIN}"
    fi
    ;;

  feature|branch)
    CACHE_SHA="${4:-$COMMIT}"
    BUCKET="${CACHE_ROOT}/${CACHE_SHA}${BUILD_SUFFIX}"

    CACHE_VALID=false
    if $MC stat --no-list "${BUCKET}/${NODE_BIN}" &>/dev/null && $MC stat --no-list "${BUCKET}/reth-bench" &>/dev/null; then
      echo "Cache hit for ${CACHE_SHA}, downloading binaries..."
      mkdir -p "${SOURCE_DIR}/target/profiling" "$(dirname "$RETH_BENCH_BIN")"
      if $MC cp "${BUCKET}/${NODE_BIN}" "${SOURCE_DIR}/target/profiling/${NODE_BIN}" && \
         $MC cp "${BUCKET}/reth-bench" "$RETH_BENCH_BIN" && \
         chmod +x "${SOURCE_DIR}/target/profiling/${NODE_BIN}" "$RETH_BENCH_BIN" && \
         verify_binary "${SOURCE_DIR}/target/profiling/${NODE_BIN}" "${COMMIT}"; then
        CACHE_VALID=true
      else
        echo "Cached feature binary is stale or download failed, rebuilding..."
      fi
    fi
    if [ "$CACHE_VALID" = false ]; then
      echo "Building feature ${NODE_BIN} (${COMMIT}) from source..."
      rustup show active-toolchain || rustup default stable
      build_node_binary
      make -C "$SOURCE_DIR" install-reth-bench
      upload_cache_artifact "${SOURCE_DIR}/target/profiling/${NODE_BIN}" "${BUCKET}/${NODE_BIN}"
      upload_cache_artifact "$RETH_BENCH_BIN" "${BUCKET}/reth-bench"
    fi
    ;;

  *)
    echo "Usage: $0 <baseline|feature> <source-dir> <commit> [cache-sha]"
    exit 1
    ;;
esac
