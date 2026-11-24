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

# rustc 1.99.1 (LLVM 23.1.1) crashes with SIGILL in X86 combineConcatVectorOps
# when building reth with -C target-cpu=native on Zen 5 runners. The fix
# (llvm/llvm-project#228522) ships in Rust 1.100; build with beta until then.
BENCH_RUST_TOOLCHAIN="${BENCH_RUST_TOOLCHAIN:-beta}"

EXTRA_FEATURES=""
if [ "${BENCH_TRACY:-off}" != "off" ]; then
  EXTRA_FEATURES="tracy,tracy-client/ondemand"
fi

install_toolchain() {
  # Both arms run in parallel; serialize the install so rustup doesn't race.
  (
    flock 9
    rustup toolchain install "$BENCH_RUST_TOOLCHAIN" --profile minimal
  ) 9>"${TMPDIR:-/tmp}/reth-bench-rustup.lock"
}

build_node_binary() {
  local features_arg=""
  local workspace_arg=""

  cd "$SOURCE_DIR"
  if [ -n "$EXTRA_FEATURES" ]; then
    features_arg="--features ${EXTRA_FEATURES}"
    workspace_arg="--workspace"
  fi

  # shellcheck disable=SC2086
  RUSTUP_TOOLCHAIN="$BENCH_RUST_TOOLCHAIN" \
    RUSTFLAGS="-C target-cpu=native -C force-frame-pointers=yes" \
    cargo build --locked --profile profiling $NODE_PKG $workspace_arg $features_arg
}

case "$MODE" in
  baseline|main)
    echo "Building baseline ${NODE_BIN} (${COMMIT}) from source for txgen benchmark..."
    install_toolchain
    build_node_binary
    ;;

  feature|branch)
    echo "Building feature ${NODE_BIN} (${COMMIT}) from source for txgen benchmark..."
    rustup show active-toolchain || rustup default stable
    install_toolchain
    build_node_binary
    ;;

  *)
    echo "Usage: $0 <baseline|feature> <source-dir> <commit>"
    exit 1
    ;;
esac
