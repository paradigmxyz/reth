#!/usr/bin/env bash
#
# Builds the node binary used by benchmark workflows.
#
# Usage: bench-txgen-build.sh <baseline|feature> <source-dir> <commit>
#
# Big-block benchmarks build reth-bb because txgen replays reth-bb's extended
# payload format.
set -euxo pipefail

# Use the runner's Git authentication configuration when fetching dependencies.
export CARGO_NET_GIT_FETCH_WITH_CLI=true

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
if [ "${BENCH_TRACY:-off}" != "off" ]; then
  EXTRA_FEATURES="tracy,tracy-client/ondemand"
fi

# Tune only the candidate binary; baseline flags remain unchanged.
if [ "$MODE" = "feature" ] || [ "$MODE" = "branch" ]; then
  case "${ALLOCATOR_BENCH_CONFIG:-default}" in
    default) ;;
    purge) EXTRA_FEATURES="${EXTRA_FEATURES:+$EXTRA_FEATURES,}mimalloc-purge" ;;
    reserve) EXTRA_FEATURES="${EXTRA_FEATURES:+$EXTRA_FEATURES,}mimalloc-reserve" ;;
    prefault) EXTRA_FEATURES="${EXTRA_FEATURES:+$EXTRA_FEATURES,}mimalloc-prefault" ;;
    1mb|2mb|4mb) EXTRA_FEATURES="${EXTRA_FEATURES:+$EXTRA_FEATURES,}mimalloc-prefault,commonware-allocator/large-objects-$ALLOCATOR_BENCH_CONFIG" ;;
    *) echo "Unknown allocator configuration" >&2; exit 1 ;;
  esac
fi

build_node_binary() {
  local features_arg=""
  local workspace_arg=""

  cd "$SOURCE_DIR"
  if [ -n "$EXTRA_FEATURES" ]; then
    features_arg="--features ${EXTRA_FEATURES}"
    if [ "${BENCH_TRACY:-off}" != "off" ]; then
      workspace_arg="--workspace"
    fi
  fi

  # shellcheck disable=SC2086
  RUSTFLAGS="-C target-cpu=native -C force-frame-pointers=yes" \
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
