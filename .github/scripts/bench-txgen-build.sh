#!/usr/bin/env bash
#
# Builds the node binary used by benchmark workflows.
#
# Usage: bench-txgen-build.sh <baseline|feature> <source-dir> <commit>
#
# Big-block benchmarks build reth-bb because txgen replays reth-bb's extended
# payload format.
set -euxo pipefail

MODE="$1"
SOURCE_DIR="$2"
COMMIT="$3"

BIG_BLOCKS="${BENCH_BIG_BLOCKS:-false}"
if [ "$BIG_BLOCKS" = "true" ]; then
  NODE_BIN="reth-bb"
  NODE_PKG="-p reth-bb"
else
  NODE_BIN="reth"
  NODE_PKG="--bin reth"
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
    cargo build --locked --profile profiling $NODE_PKG $workspace_arg $features_arg
}

case "$MODE" in
  baseline|main)
    echo "Building baseline ${NODE_BIN} (${COMMIT}) from source for txgen benchmark..."
    build_node_binary
    ;;

  feature|branch)
    echo "Building feature ${NODE_BIN} (${COMMIT}) from source for txgen benchmark..."
    rustup show active-toolchain || rustup default stable
    build_node_binary
    ;;

  *)
    echo "Usage: $0 <baseline|feature> <source-dir> <commit>"
    exit 1
    ;;
esac
