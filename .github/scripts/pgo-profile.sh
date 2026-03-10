#!/usr/bin/env bash
#
# Collects PGO profiles by running reth with real block execution via reth-bench.
#
# Builds an instrumented reth binary, starts a node, runs reth-bench to
# execute blocks, then merges the resulting .profraw files into a single
# .profdata for use with -Cprofile-use.
#
# This script is runner-agnostic — all environment setup (snapshots, mounts,
# datadir paths) must be done by the caller.
#
# Required environment variables:
#   DATADIR    - Path to reth datadir (must already contain chain data)
#   RPC_URL    - Source RPC URL for reth-bench to fetch payloads from
#
# Optional environment variables:
#   PGO_BLOCKS - Number of blocks to execute for profiling (default: 10)
#   PROFILE    - Cargo profile (default: maxperf-symbols)
#   FEATURES   - Cargo features (default: jemalloc,asm-keccak,min-debug-logs)
#   TARGET     - Target triple (default: auto-detected from rustc)
#   EXTRA_RUSTFLAGS - Additional RUSTFLAGS (e.g. -C target-cpu=x86-64-v3)
#
# Output:
#   target/pgo-profiles/merged.profdata
set -euo pipefail

gha_section_start() {
    local title="$1"
    if [ -n "${GITHUB_ACTIONS:-}" ]; then
        echo "::group::$title"
    else
        echo ""
        echo "=== $title ==="
    fi
}

gha_section_end() {
    if [ -n "${GITHUB_ACTIONS:-}" ]; then
        echo "::endgroup::"
    fi
}

cd "$(dirname "$0")/../.."

: "${DATADIR:?DATADIR must be set to the reth data directory}"
: "${RPC_URL:?RPC_URL must be set}"

PGO_BLOCKS="${PGO_BLOCKS:-10}"
PROFILE="${PROFILE:-maxperf-symbols}"
FEATURES="${FEATURES:-jemalloc,asm-keccak,min-debug-logs}"
TARGET="${TARGET:-$(rustc -Vv | grep host | cut -d' ' -f2)}"
PGO_DIR="$PWD/target/pgo-profiles"

if [[ "$PROFILE" == dev ]]; then
    PROFILE_DIR=debug
else
    PROFILE_DIR=$PROFILE
fi

gha_section_start "PGO Profile Collection"
echo "Blocks: $PGO_BLOCKS"
echo "Datadir: $DATADIR"
echo "RPC URL: $RPC_URL"
echo "Profile: $PROFILE"
echo "Features: $FEATURES"
echo "Target: $TARGET"
gha_section_end

# Clean old profiles
rm -rf "$PGO_DIR"
mkdir -p "$PGO_DIR"

# Build instrumented binary
gha_section_start "Building PGO-instrumented binary"
rustup component add llvm-tools-preview
RUSTFLAGS="-Cprofile-generate=$PGO_DIR ${EXTRA_RUSTFLAGS:-}" \
    cargo build --profile "$PROFILE" --features "$FEATURES" \
    --manifest-path bin/reth/Cargo.toml --bin reth --locked \
    --target "$TARGET"

RETH_BIN="$PWD/target/$TARGET/$PROFILE_DIR/reth"
echo "Instrumented binary: $RETH_BIN"
ls -lh "$RETH_BIN"
gha_section_end

# Also build reth-bench (non-instrumented)
gha_section_start "Building reth-bench"
cargo build --profile "$PROFILE" --features "$FEATURES" \
    --manifest-path bin/reth-bench/Cargo.toml --bin reth-bench --locked
RETH_BENCH_BIN="$(find target -name reth-bench -type f -executable | head -1)"
echo "reth-bench binary: $RETH_BENCH_BIN"
gha_section_end

# Cleanup handler — stop reth on exit
RETH_PID=
cleanup() {
    if [ -n "${RETH_PID:-}" ] && kill -0 "$RETH_PID" 2>/dev/null; then
        echo "Stopping reth (pid $RETH_PID)..."
        sudo kill "$RETH_PID" 2>/dev/null || true
        for i in $(seq 1 60); do
            sudo kill -0 "$RETH_PID" 2>/dev/null || break
            if [ $((i % 10)) -eq 0 ]; then
                echo "Waiting for reth to flush profiles... (${i}s)"
            fi
            sleep 1
        done
        sudo kill -9 "$RETH_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Start reth node
gha_section_start "Running reth-bench workload"
echo "Starting reth node..."
sudo "$RETH_BIN" node \
    --datadir "$DATADIR" \
    --log.file.directory /tmp/reth-pgo-logs \
    --engine.accept-execution-requests-hash \
    --http --http.port 8545 \
    --authrpc.port 8551 \
    --disable-discovery --no-persist-peers \
    > /tmp/reth-pgo-node.log 2>&1 &
RETH_PID=$!

# Wait for RPC to be ready
echo "Waiting for reth RPC..."
for i in $(seq 1 120); do
    if curl -sf http://127.0.0.1:8545 -X POST \
        -H 'Content-Type: application/json' \
        -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
        > /dev/null 2>&1; then
        echo "reth is ready after ${i}s"
        break
    fi
    if [ "$i" -eq 120 ]; then
        echo "::error::reth failed to start within 120s"
        cat /tmp/reth-pgo-node.log
        exit 1
    fi
    sleep 1
done

# Run reth-bench to execute blocks and generate PGO profiles
echo "Running reth-bench for $PGO_BLOCKS blocks..."
"$RETH_BENCH_BIN" new-payload-fcu \
    --rpc-url "$RPC_URL" \
    --engine-rpc-url http://127.0.0.1:8551 \
    --jwt-secret "$DATADIR/jwt.hex" \
    --advance "$PGO_BLOCKS" \
    --reth-new-payload 2>&1 | sed -u "s/^/[bench] /"

# Stop reth gracefully to flush profraw files
echo "Stopping reth..."
sudo kill "$RETH_PID" 2>/dev/null || true
for i in $(seq 1 60); do
    sudo kill -0 "$RETH_PID" 2>/dev/null || break
    sleep 1
done
sudo kill -9 "$RETH_PID" 2>/dev/null || true
RETH_PID=
gha_section_end

# Fix ownership (reth ran as root)
sudo chown -R "$(id -un):$(id -gn)" "$PGO_DIR" 2>/dev/null || true

# Merge profiles
gha_section_start "Merging PGO profiles"
PROFRAW_COUNT=$(find "$PGO_DIR" -name '*.profraw' | wc -l)
echo "Found $PROFRAW_COUNT .profraw files"

if [ "$PROFRAW_COUNT" -eq 0 ]; then
    echo "::error::No .profraw files found — instrumented binary did not produce profiles"
    exit 1
fi

LLVM_PROFDATA=$(find "$(rustc --print sysroot)" -name llvm-profdata -type f | head -1)
if [ -z "$LLVM_PROFDATA" ]; then
    echo "::error::llvm-profdata not found"
    exit 1
fi

"$LLVM_PROFDATA" merge -o "$PGO_DIR/merged.profdata" "$PGO_DIR"/*.profraw
ls -lh "$PGO_DIR/merged.profdata"
gha_section_end

gha_section_start "PGO Profile Collection Complete"
echo "Profile: $PGO_DIR/merged.profdata"
gha_section_end
