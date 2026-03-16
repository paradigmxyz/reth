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
#   PGO_BLOCKS      - Number of blocks for PGO profiling (default: 20)
#   BOLT_BLOCKS     - Number of blocks for BOLT profiling (default: 20)
#   SKIP_BOLT       - Temporarily skip BOLT phases (default: false)
#   STRIP_SYMBOLS   - Strip debug symbols from output binary (default: true)
#   COLLECT_PGO_ONLY - Stop after producing merged.profdata (default: false)
#   PGO_PROFDATA    - Path to pre-collected merged.profdata (optional)
#   PROFILE         - Cargo profile (default: maxperf-symbols)
#   FEATURES        - Cargo features (default: jemalloc,asm-keccak,min-debug-logs)
#   TARGET          - Target triple (default: auto-detected)
#   EXTRA_RUSTFLAGS - Additional RUSTFLAGS (e.g. -C target-cpu=x86-64-v3)
#
# Output:
#   target/$PROFILE_DIR/reth  — final optimized binary
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

# ── Configuration ──────────────────────────────────────────────────────────────
PGO_BLOCKS="${PGO_BLOCKS:-20}"
BOLT_BLOCKS="${BOLT_BLOCKS:-20}"
SKIP_BOLT="${SKIP_BOLT:-false}"
STRIP_SYMBOLS="${STRIP_SYMBOLS:-true}"
COLLECT_PGO_ONLY="${COLLECT_PGO_ONLY:-false}"
PROFILE="${PROFILE:-maxperf-symbols}"
FEATURES="${FEATURES:-jemalloc,asm-keccak,min-debug-logs}"
TARGET="${TARGET:-$(rustc -Vv | grep host | cut -d' ' -f2)}"
BASE_RUSTFLAGS="${RUSTFLAGS:-}"
EXTRA_RUSTFLAGS="${EXTRA_RUSTFLAGS:-}"
COMBINED_RUSTFLAGS="$BASE_RUSTFLAGS $EXTRA_RUSTFLAGS"
PGO_PROFDATA="${PGO_PROFDATA:-}"
DATADIR="${DATADIR:-}"
RPC_URL="${RPC_URL:-}"

SKIP_BOLT_BOOL=false
if [[ "${SKIP_BOLT,,}" == "true" || "$SKIP_BOLT" == "1" ]]; then
    SKIP_BOLT_BOOL=true
fi

STRIP_SYMBOLS_BOOL=false
if [[ "${STRIP_SYMBOLS,,}" == "true" || "$STRIP_SYMBOLS" == "1" ]]; then
    STRIP_SYMBOLS_BOOL=true
fi

COLLECT_PGO_ONLY_BOOL=false
if [[ "${COLLECT_PGO_ONLY,,}" == "true" || "$COLLECT_PGO_ONLY" == "1" ]]; then
    COLLECT_PGO_ONLY_BOOL=true
fi

USE_PRECOLLECTED_PGO=false
if [ -n "$PGO_PROFDATA" ]; then
    if [ ! -f "$PGO_PROFDATA" ]; then
        echo "error: PGO_PROFDATA points to a missing file: $PGO_PROFDATA"
        exit 1
    fi
    USE_PRECOLLECTED_PGO=true
fi

NEEDS_BENCH_WORKLOAD=true
if [ "$USE_PRECOLLECTED_PGO" = true ] && [ "$SKIP_BOLT_BOOL" = true ]; then
    NEEDS_BENCH_WORKLOAD=false
fi

if [ "$NEEDS_BENCH_WORKLOAD" = true ]; then
    : "${DATADIR:?DATADIR must be set to the reth data directory}"
    : "${RPC_URL:?RPC_URL must be set}"
fi

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

gha_section_start "Full PGO+BOLT Build"
echo "Binary:      reth"
echo "Manifest:    $MANIFEST_PATH"
echo "Target:      $TARGET"
echo "Profile:     $PROFILE"
echo "Features:    $FEATURES"
echo "LLVM:        $LLVM_VERSION"
echo "PGO blocks:  $PGO_BLOCKS"
echo "BOLT blocks: $BOLT_BLOCKS"
echo "Skip BOLT:   $SKIP_BOLT"
echo "Strip symbols: $STRIP_SYMBOLS"
echo "Collect only: $COLLECT_PGO_ONLY"
echo "PGO profdata: ${PGO_PROFDATA:-<collect with reth-bench>}"
echo "RUSTFLAGS:   ${BASE_RUSTFLAGS:-<unset>}"
echo "EXTRA_RUSTFLAGS: ${EXTRA_RUSTFLAGS:-<unset>}"
if [ "$NEEDS_BENCH_WORKLOAD" = true ]; then
    echo "Datadir:     $DATADIR"
    echo "RPC URL:     $RPC_URL"
else
    echo "Datadir:     <not required>"
    echo "RPC URL:     <not required>"
fi
gha_section_end

# ── Prerequisites ──────────────────────────────────────────────────────────────
gha_section_start "Installing prerequisites"
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
if [ "$SKIP_BOLT_BOOL" = true ]; then
    echo "Skipping BOLT installation (SKIP_BOLT=$SKIP_BOLT)"
else
    install_bolt
fi
gha_section_end

if [ "$NEEDS_BENCH_WORKLOAD" = true ]; then
    # Build reth-bench once (non-instrumented) — reused for both phases.
    gha_section_start "Building reth-bench"
    RUSTFLAGS="$COMBINED_RUSTFLAGS" \
        cargo build --profile "$PROFILE" --features "$FEATURES" \
        --manifest-path bin/reth-bench/Cargo.toml --bin reth-bench --locked
    RETH_BENCH_BIN="$(find target -name reth-bench -type f -executable | head -1)"
    echo "reth-bench: $RETH_BENCH_BIN"
    gha_section_end
else
    gha_section_start "Building reth-bench"
    echo "Skipping reth-bench build (pre-collected PGO with SKIP_BOLT=true)"
    gha_section_end
fi

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

publish_binary() {
    local source_bin="$1"
    for out in "target/$TARGET/$PROFILE_DIR" "target/$PROFILE_DIR"; do
        local destination="$out/reth"
        mkdir -p "$out"
        # Skip copying when source and destination resolve to the same inode.
        if [ -e "$destination" ] && [ "$source_bin" -ef "$destination" ]; then
            continue
        fi
        cp "$source_bin" "$destination"
    done
}

if [ "$USE_PRECOLLECTED_PGO" = true ]; then
    gha_section_start "Phase 1: Using Pre-Collected PGO Profile"
    rm -rf "$PGO_DIR"
    mkdir -p "$PGO_DIR"
    cp "$PGO_PROFDATA" "$PGO_DIR/merged.profdata"
    echo "Using pre-collected profile: $PGO_PROFDATA"
    echo "PGO profile: $PGO_DIR/merged.profdata ($(ls -lh "$PGO_DIR/merged.profdata" | awk '{print $5}'))"
    gha_section_end
else
    # ── Phase 1: PGO profile collection ───────────────────────────────────────
    gha_section_start "Phase 1: PGO Profile Collection"

    rm -rf "$PGO_DIR"
    mkdir -p "$PGO_DIR"

    echo "Building PGO-instrumented binary..."
    RUSTFLAGS="-Cprofile-generate=$PGO_DIR -Crelocation-model=pic $COMBINED_RUSTFLAGS" \
        cargo build "${CARGO_ARGS[@]}" --target "$TARGET"

    PGO_RETH_BIN="$PWD/target/$TARGET/$PROFILE_DIR/reth"
    echo "Instrumented binary: $PGO_RETH_BIN ($(ls -lh "$PGO_RETH_BIN" | awk '{print $5}'))"

    run_bench_workload "$PGO_RETH_BIN" "$PGO_BLOCKS" "pgo"

    # Fix ownership if reth ran as root.
    sudo chown -R "$(id -un):$(id -gn)" "$PGO_DIR" 2>/dev/null || true

    # Merge PGO profiles.
    echo "Merging PGO profiles..."
    PROFRAW_COUNT=$(find "$PGO_DIR" -name '*.profraw' | wc -l)
    echo "Found $PROFRAW_COUNT .profraw files"
    if [ "$PROFRAW_COUNT" -eq 0 ]; then
        echo "error: no .profraw files — instrumented binary did not produce profiles"
        exit 1
    fi
    "$LLVM_PROFDATA" merge -o "$PGO_DIR/merged.profdata" "$PGO_DIR"/*.profraw
    echo "PGO profile: $PGO_DIR/merged.profdata ($(ls -lh "$PGO_DIR/merged.profdata" | awk '{print $5}'))"
    gha_section_end
fi

if [ "$COLLECT_PGO_ONLY_BOOL" = true ]; then
    gha_section_start "PGO Collection Complete"
    echo "COLLECT_PGO_ONLY=true, skipping PGO/BOLT optimized binary build"
    echo "Profile: $PGO_DIR/merged.profdata"
    gha_section_end
    exit 0
fi

if [ "$SKIP_BOLT_BOOL" = true ]; then
    gha_section_start "BOLT Phase Skipped"
    echo "SKIP_BOLT=$SKIP_BOLT, building PGO-only binary"
    echo "Building PGO-optimized binary..."
    RUSTFLAGS="-Cprofile-use=$PGO_DIR/merged.profdata $COMBINED_RUSTFLAGS" \
        cargo build "${CARGO_ARGS[@]}" --target "$TARGET"

    BUILT_BIN="$PWD/target/$TARGET/$PROFILE_DIR/reth"
    if [ "$STRIP_SYMBOLS_BOOL" = true ]; then
        echo "Stripping debug symbols..."
        strip "$BUILT_BIN"
    else
        echo "Skipping strip (STRIP_SYMBOLS=$STRIP_SYMBOLS)"
    fi
    publish_binary "$BUILT_BIN"
    gha_section_end
else
    # ── Phase 2: BOLT profile collection (with PGO) ──────────────────────────
    gha_section_start "Phase 2: BOLT Profile Collection (with PGO)"

    rm -rf "$BOLT_DIR"
    mkdir -p "$BOLT_DIR"

    echo "Building BOLT-instrumented binary with PGO..."
    # --emit-relocs preserves relocation entries in the binary, required by llvm-bolt -instrument
    RUSTFLAGS="-Cprofile-use=$PGO_DIR/merged.profdata -Clink-arg=-Wl,--emit-relocs $COMBINED_RUSTFLAGS" \
        cargo build "${CARGO_ARGS[@]}" --target "$TARGET"

    # Instrument with BOLT
    BUILT_BIN="$PWD/target/$TARGET/$PROFILE_DIR/reth"
    BOLT_INSTRUMENTED_BIN="$BUILT_BIN-bolt-instrumented"

    echo "Instrumenting binary with BOLT..."
    # --skip-funcs: skip compiler-generated drop_in_place functions that BOLT can't handle
    # as split functions in relocation mode (triggered by --emit-relocs)
    llvm-bolt "$BUILT_BIN" \
        -instrument \
        --instrumentation-file-append-pid \
        --instrumentation-file="$BOLT_DIR/prof" \
        --skip-funcs='.*drop_in_place.*' \
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
    gha_section_end

    # ── Phase 3: Final optimized build ───────────────────────────────────────
    gha_section_start "Phase 3: Final PGO+BOLT Optimized Build"

    echo "Building PGO-optimized binary..."
    # --emit-relocs preserves relocation entries in the binary, required by llvm-bolt for code reordering
    RUSTFLAGS="-Cprofile-use=$PGO_DIR/merged.profdata -Clink-arg=-Wl,--emit-relocs $COMBINED_RUSTFLAGS" \
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
        -use-gnu-stack \
        --skip-funcs='.*drop_in_place.*'

    if [ "$STRIP_SYMBOLS_BOOL" = true ]; then
        echo "Stripping debug symbols..."
        strip "$OPTIMIZED_BIN"
    else
        echo "Skipping strip (STRIP_SYMBOLS=$STRIP_SYMBOLS)"
    fi
    publish_binary "$OPTIMIZED_BIN"
    gha_section_end
fi

gha_section_start "Build Complete"
ls -lh "target/$PROFILE_DIR/reth"
echo "Output: target/$PROFILE_DIR/reth"
gha_section_end
