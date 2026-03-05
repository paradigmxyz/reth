#!/usr/bin/env bash
#
# Full PGO+BOLT optimized build for reth using real reth-bench workloads.
#
# Phases:
#   1. Build PGO-instrumented reth, run reth-bench → collect PGO profiles
#   2. Build BOLT-instrumented reth (with PGO), run reth-bench → collect BOLT profiles
#   3. Build final PGO+BOLT optimized binary
#
# Required environment variables:
#   DATADIR    - Path to reth datadir (must already contain chain data)
#   RPC_URL    - Source RPC URL for reth-bench to fetch payloads from
#
# Optional environment variables:
#   PGO_BLOCKS      - Number of blocks for PGO profiling (default: 10)
#   BOLT_BLOCKS     - Number of blocks for BOLT profiling (default: 10)
#   PROFILE         - Cargo profile (default: maxperf-symbols)
#   FEATURES        - Cargo features (default: jemalloc,asm-keccak,min-debug-logs)
#   TARGET          - Target triple (default: auto-detected)
#   EXTRA_RUSTFLAGS - Additional RUSTFLAGS (e.g. -C target-cpu=x86-64-v3)
#
# Output:
#   target/$PROFILE_DIR/reth  — final optimized binary
set -euo pipefail

cd "$(dirname "$0")/../.."

# ── Configuration ──────────────────────────────────────────────────────────────
: "${DATADIR:?DATADIR must be set to the reth data directory}"
: "${RPC_URL:?RPC_URL must be set}"

PGO_BLOCKS="${PGO_BLOCKS:-10}"
BOLT_BLOCKS="${BOLT_BLOCKS:-10}"
PROFILE="${PROFILE:-maxperf-symbols}"
FEATURES="${FEATURES:-jemalloc,asm-keccak,min-debug-logs}"
TARGET="${TARGET:-$(rustc -Vv | grep host | cut -d' ' -f2)}"

if [[ "$PROFILE" == dev ]]; then
    PROFILE_DIR=debug
else
    PROFILE_DIR=$PROFILE
fi

MANIFEST_PATH="bin/reth"

LLVM_VERSION=$(rustc -Vv | grep -oP 'LLVM version: \K\d+')
PGO_DIR="$PWD/target/pgo-profiles"
BOLT_DIR="$PWD/target/bolt-profiles"
CARGO_ARGS=(--profile "$PROFILE" --features "$FEATURES" --manifest-path "$MANIFEST_PATH/Cargo.toml" --bin "reth" --locked)

# Enable debug symbols for BOLT (requires symbols to reorder code).
# Strip them at the end.
PROFILE_UPPER=$(echo "$PROFILE" | tr '[:lower:]-' '[:upper:]_')
export "CARGO_PROFILE_${PROFILE_UPPER}_STRIP=debuginfo"

echo "=== Full PGO+BOLT Build ==="
echo "Binary:      reth"
echo "Manifest:    $MANIFEST_PATH"
echo "Target:      $TARGET"
echo "Profile:     $PROFILE"
echo "Features:    $FEATURES"
echo "LLVM:        $LLVM_VERSION"
echo "PGO blocks:  $PGO_BLOCKS"
echo "BOLT blocks: $BOLT_BLOCKS"
echo "Datadir:     $DATADIR"
echo "RPC URL:     $RPC_URL"

# ── Prerequisites ──────────────────────────────────────────────────────────────
echo "=== Installing prerequisites ==="
rustup component add llvm-tools-preview

LLVM_PROFDATA=$(find "$(rustc --print sysroot)" -name llvm-profdata -type f | head -1)
if [ -z "$LLVM_PROFDATA" ]; then
    echo "error: llvm-profdata not found"
    exit 1
fi

install_bolt() {
    if command -v llvm-bolt &>/dev/null; then
        echo "BOLT already installed"
        return
    fi
    echo "Installing BOLT from apt.llvm.org..."
    wget -qO- https://apt.llvm.org/llvm-snapshot.gpg.key | sudo tee /etc/apt/trusted.gpg.d/apt.llvm.org.asc >/dev/null
    CODENAME=$(lsb_release -cs)
    echo "deb http://apt.llvm.org/$CODENAME/ llvm-toolchain-$CODENAME-$LLVM_VERSION main" | sudo tee /etc/apt/sources.list.d/llvm.list >/dev/null
    sudo apt-get update -qq
    sudo apt-get install -y -qq "bolt-$LLVM_VERSION"
    sudo ln -sf "/usr/bin/llvm-bolt-$LLVM_VERSION" /usr/local/bin/llvm-bolt
    sudo ln -sf "/usr/bin/merge-fdata-$LLVM_VERSION" /usr/local/bin/merge-fdata
}
install_bolt

# Build reth-bench once (non-instrumented) — reused for both phases
echo "=== Building reth-bench ==="
cargo build --profile "$PROFILE" --features "$FEATURES" \
    --manifest-path bin/reth-bench/Cargo.toml --bin reth-bench --locked
RETH_BENCH_BIN="$(find target -name reth-bench -type f -executable | head -1)"
echo "reth-bench: $RETH_BENCH_BIN"

# ── Helpers ────────────────────────────────────────────────────────────────────
RETH_PID=
cleanup() {
    if [ -n "${RETH_PID:-}" ] && kill -0 "$RETH_PID" 2>/dev/null; then
        echo "Stopping reth (pid $RETH_PID)..."
        sudo kill "$RETH_PID" 2>/dev/null || true
        for i in $(seq 1 60); do
            sudo kill -0 "$RETH_PID" 2>/dev/null || break
            if [ $((i % 10)) -eq 0 ]; then
                echo "  waiting... (${i}s)"
            fi
            sleep 1
        done
        sudo kill -9 "$RETH_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# Start reth, wait for RPC, run reth-bench, then stop reth.
# Arguments: $1 = reth binary path, $2 = number of blocks, $3 = log label
run_bench_workload() {
    local reth_bin="$1" blocks="$2" label="$3"
    local http_port=8545 authrpc_port=8551

    echo "--- Starting reth ($label) ---"
    sudo "$reth_bin" node \
        --datadir "$DATADIR" \
        --log.file.directory "/tmp/reth-${label}-logs" \
        --engine.accept-execution-requests-hash \
        --http --http.port "$http_port" \
        --authrpc.port "$authrpc_port" \
        --disable-discovery --no-persist-peers \
        > "/tmp/reth-${label}.log" 2>&1 &
    RETH_PID=$!

    echo "Waiting for reth RPC..."
    for i in $(seq 1 120); do
        if curl -sf "http://127.0.0.1:$http_port" -X POST \
            -H 'Content-Type: application/json' \
            -d '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' \
            > /dev/null 2>&1; then
            echo "reth is ready after ${i}s"
            break
        fi
        if [ "$i" -eq 120 ]; then
            echo "error: reth failed to start within 120s"
            cat "/tmp/reth-${label}.log"
            exit 1
        fi
        sleep 1
    done

    echo "Running reth-bench ($blocks blocks)..."
    "$RETH_BENCH_BIN" new-payload-fcu \
        --rpc-url "$RPC_URL" \
        --engine-rpc-url "http://127.0.0.1:$authrpc_port" \
        --jwt-secret "$DATADIR/jwt.hex" \
        --advance "$blocks" \
        --reth-new-payload 2>&1 | sed -u "s/^/[$label] /"

    echo "Stopping reth ($label)..."
    sudo kill "$RETH_PID" 2>/dev/null || true
    for i in $(seq 1 60); do
        sudo kill -0 "$RETH_PID" 2>/dev/null || break
        sleep 1
    done
    sudo kill -9 "$RETH_PID" 2>/dev/null || true
    RETH_PID=
}

# ── Phase 1: PGO profile collection ───────────────────────────────────────────
echo ""
echo "============================================================"
echo "  Phase 1: PGO Profile Collection"
echo "============================================================"

rm -rf "$PGO_DIR"
mkdir -p "$PGO_DIR"

echo "Building PGO-instrumented binary..."
RUSTFLAGS="-Cprofile-generate=$PGO_DIR ${EXTRA_RUSTFLAGS:-}" \
    cargo build "${CARGO_ARGS[@]}" --target "$TARGET"

PGO_RETH_BIN="$PWD/target/$TARGET/$PROFILE_DIR/reth"
echo "Instrumented binary: $PGO_RETH_BIN ($(ls -lh "$PGO_RETH_BIN" | awk '{print $5}'))"

run_bench_workload "$PGO_RETH_BIN" "$PGO_BLOCKS" "pgo"

# Fix ownership if reth ran as root
sudo chown -R "$(id -un):$(id -gn)" "$PGO_DIR" 2>/dev/null || true

# Merge PGO profiles
echo "Merging PGO profiles..."
PROFRAW_COUNT=$(find "$PGO_DIR" -name '*.profraw' | wc -l)
echo "Found $PROFRAW_COUNT .profraw files"
if [ "$PROFRAW_COUNT" -eq 0 ]; then
    echo "error: no .profraw files — instrumented binary did not produce profiles"
    exit 1
fi
"$LLVM_PROFDATA" merge -o "$PGO_DIR/merged.profdata" "$PGO_DIR"/*.profraw
echo "PGO profile: $PGO_DIR/merged.profdata ($(ls -lh "$PGO_DIR/merged.profdata" | awk '{print $5}'))"

# ── Phase 2: BOLT profile collection (with PGO) ──────────────────────────────
echo ""
echo "============================================================"
echo "  Phase 2: BOLT Profile Collection (with PGO)"
echo "============================================================"

rm -rf "$BOLT_DIR"
mkdir -p "$BOLT_DIR"

echo "Building BOLT-instrumented binary with PGO..."
RUSTFLAGS="-Cprofile-use=$PGO_DIR/merged.profdata ${EXTRA_RUSTFLAGS:-}" \
    cargo build "${CARGO_ARGS[@]}" --target "$TARGET"

# Instrument with BOLT
BUILT_BIN="$PWD/target/$TARGET/$PROFILE_DIR/reth"
BOLT_INSTRUMENTED_BIN="$BUILT_BIN-bolt-instrumented"

echo "Instrumenting binary with BOLT..."
llvm-bolt "$BUILT_BIN" \
    -instrument \
    --instrumentation-file-append-pid \
    --instrumentation-file="$BOLT_DIR/prof" \
    -o "$BOLT_INSTRUMENTED_BIN"
echo "BOLT-instrumented binary: $BOLT_INSTRUMENTED_BIN ($(ls -lh "$BOLT_INSTRUMENTED_BIN" | awk '{print $5}'))"

run_bench_workload "$BOLT_INSTRUMENTED_BIN" "$BOLT_BLOCKS" "bolt"

# Fix ownership for BOLT profiles
sudo chown -R "$(id -un):$(id -gn)" "$BOLT_DIR" 2>/dev/null || true

# Merge BOLT profiles
echo "Merging BOLT profiles..."
FDATA_COUNT=$(find "$BOLT_DIR" -name '*.fdata' | wc -l)
echo "Found $FDATA_COUNT .fdata files"
if [ "$FDATA_COUNT" -eq 0 ]; then
    echo "error: no .fdata files — BOLT-instrumented binary did not produce profiles"
    exit 1
fi
merge-fdata "$BOLT_DIR"/*.fdata > "$BOLT_DIR/merged.fdata"
echo "BOLT profile: $BOLT_DIR/merged.fdata ($(ls -lh "$BOLT_DIR/merged.fdata" | awk '{print $5}'))"

# ── Phase 3: Final optimized build ───────────────────────────────────────────
echo ""
echo "============================================================"
echo "  Phase 3: Final PGO+BOLT Optimized Build"
echo "============================================================"

echo "Building PGO-optimized binary..."
RUSTFLAGS="-Cprofile-use=$PGO_DIR/merged.profdata ${EXTRA_RUSTFLAGS:-}" \
    cargo build "${CARGO_ARGS[@]}" --target "$TARGET"

BUILT_BIN="$PWD/target/$TARGET/$PROFILE_DIR/reth"
OPTIMIZED_BIN="$BUILT_BIN-bolt-optimized"

echo "Optimizing with BOLT..."
llvm-bolt "$BUILT_BIN" \
    -o "$OPTIMIZED_BIN" \
    --data "$BOLT_DIR/merged.fdata" \
    -reorder-blocks=ext-tsp \
    -reorder-functions=cdsort \
    -split-functions \
    -split-all-cold \
    -dyno-stats \
    -icf=1 \
    -use-gnu-stack

echo "Stripping debug symbols..."
strip "$OPTIMIZED_BIN"

# Copy to expected output locations
for out in "target/$TARGET/$PROFILE_DIR" "target/$PROFILE_DIR"; do
    mkdir -p "$out"
    cp "$OPTIMIZED_BIN" "$out/reth"
done

echo ""
echo "============================================================"
echo "  Build Complete"
echo "============================================================"
ls -lh "target/$PROFILE_DIR/reth"
echo "Output: target/$PROFILE_DIR/reth"
