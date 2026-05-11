#!/usr/bin/env bash
#
# Builds the node binary for the txgen-backed PR benchmark path.
#
# Usage: bench-txgen-build.sh <baseline|feature> <source-dir> <commit>
#
# This intentionally does not build or install reth-bench. Big-block benchmarks
# still use the legacy reth-bench path because txgen does not yet replay the
# reth-bb payload/env-switch/BAL format.
set -euxo pipefail

MODE="$1"
SOURCE_DIR="$2"
COMMIT="$3"

if [ "${BENCH_BIG_BLOCKS:-false}" = "true" ]; then
  echo "::error::txgen path does not support big-block benchmarks yet; use the reth-bench driver"
  exit 1
fi

EXTRA_FEATURES=""
EXTRA_RUSTFLAGS=""
if [ "${BENCH_TRACY:-off}" != "off" ]; then
  EXTRA_FEATURES="tracy,tracy-client/ondemand"
  EXTRA_RUSTFLAGS=" -C force-frame-pointers=yes"
fi

build_node_binary() {
  local features_arg=""
  local workspace_arg=""

  cd "$SOURCE_DIR"
  if [ -n "$EXTRA_FEATURES" ]; then
    features_arg="--features ${EXTRA_FEATURES}"
    workspace_arg="--workspace"
  fi

  # shellcheck disable=SC2086
  RUSTFLAGS="-C target-cpu=native${EXTRA_RUSTFLAGS}" \
    cargo build --locked --profile profiling --bin reth $workspace_arg $features_arg
}

case "$MODE" in
  baseline|main)
    echo "Building baseline reth (${COMMIT}) from source for txgen benchmark..."
    build_node_binary
    ;;

  feature|branch)
    echo "Building feature reth (${COMMIT}) from source for txgen benchmark..."
    rustup show active-toolchain || rustup default stable
    build_node_binary
    ;;

  *)
    echo "Usage: $0 <baseline|feature> <source-dir> <commit>"
    exit 1
    ;;
esac
