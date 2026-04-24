#!/usr/bin/env bash
#
# Builds reth binaries for benchmarking from local source only.
#
# Usage: bench-reth-build.sh <baseline|feature> <source-dir> <commit>
#
#   baseline — build the baseline binary at <commit> (merge-base)
#              source-dir must be checked out at <commit>
#   feature  — build the candidate binary + reth-bench at <commit>
#              source-dir must be checked out at <commit>
#
# Outputs:
#   baseline: <source-dir>/target/profiling/reth (or reth-bb if BENCH_BIG_BLOCKS=true)
#   feature:  <source-dir>/target/profiling/reth (or reth-bb), reth-bench installed to cargo bin
#
# Optional env: BENCH_BIG_BLOCKS (true/false) — build reth-bb instead of reth
set -euxo pipefail

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
    echo "Building baseline ${NODE_BIN} (${COMMIT}) from source..."
    build_node_binary
    ;;

  feature|branch)
    echo "Building feature ${NODE_BIN} (${COMMIT}) from source..."
    rustup show active-toolchain || rustup default stable
    build_node_binary
    make -C "$SOURCE_DIR" install-reth-bench
    ;;

  *)
    echo "Usage: $0 <baseline|feature> <source-dir> <commit>"
    exit 1
    ;;
esac
