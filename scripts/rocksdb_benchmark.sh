#!/bin/bash
# RocksDB vs MDBX Benchmark Script for Hoodi Network
# Usage: ./scripts/rocksdb_benchmark.sh [mdbx|rocks] <start_block> <num_blocks>

set -euo pipefail

# Configuration
RETH_BIN="${RETH_BIN:-./target/release/reth}"
RETH_BENCH_BIN="${RETH_BENCH_BIN:-./target/release/reth-bench}"
CHAIN="${CHAIN:-hoodi}"
JWT_SECRET="${JWT_SECRET:-/tmp/jwt.hex}"
RPC_URL="${RPC_URL:-http://localhost:8545}"  # Archive node for block data
OUTPUT_DIR="${OUTPUT_DIR:-./benchmark-results}"

# Ports
MDBX_AUTH_PORT=8551
MDBX_HTTP_PORT=8545
MDBX_WS_PORT=8546
MDBX_METRICS_PORT=9100

ROCKS_AUTH_PORT=8552
ROCKS_HTTP_PORT=8547
ROCKS_WS_PORT=8548
ROCKS_METRICS_PORT=9101

usage() {
    echo "Usage: $0 [mdbx|rocks|both] <start_block> <num_blocks>"
    echo ""
    echo "Examples:"
    echo "  $0 mdbx 1000000 1000    # Run MDBX benchmark from block 1M for 1000 blocks"
    echo "  $0 rocks 1000000 1000   # Run RocksDB benchmark from block 1M for 1000 blocks"
    echo "  $0 both 1000000 1000    # Run both benchmarks sequentially"
    echo ""
    echo "Environment variables:"
    echo "  RETH_BIN       - Path to reth binary (default: ./target/release/reth)"
    echo "  RETH_BENCH_BIN - Path to reth-bench binary (default: ./target/release/reth-bench)"
    echo "  CHAIN          - Chain name (default: hoodi)"
    echo "  JWT_SECRET     - Path to JWT secret file (default: /tmp/jwt.hex)"
    echo "  RPC_URL        - Archive node RPC URL for block data (default: http://localhost:8545)"
    echo "  OUTPUT_DIR     - Output directory for results (default: ./benchmark-results)"
    echo "  HOODI_DATADIR_MDBX  - Data directory for MDBX benchmark"
    echo "  HOODI_DATADIR_ROCKS - Data directory for RocksDB benchmark"
    exit 1
}

# Check arguments
if [[ $# -lt 3 ]]; then
    usage
fi

MODE=$1
START_BLOCK=$2
NUM_BLOCKS=$3
END_BLOCK=$((START_BLOCK + NUM_BLOCKS))

# Create output directory with timestamp
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULT_DIR="${OUTPUT_DIR}/${TIMESTAMP}"
mkdir -p "$RESULT_DIR"

echo "=== RocksDB vs MDBX Benchmark ==="
echo "Mode: $MODE"
echo "Block range: $START_BLOCK -> $END_BLOCK ($NUM_BLOCKS blocks)"
echo "Results: $RESULT_DIR"
echo ""

# Generate JWT secret if not exists
if [[ ! -f "$JWT_SECRET" ]]; then
    echo "Generating JWT secret at $JWT_SECRET"
    openssl rand -hex 32 > "$JWT_SECRET"
fi

# Check binaries exist
if [[ ! -x "$RETH_BIN" ]]; then
    echo "Error: reth binary not found at $RETH_BIN"
    echo "Build with: cargo build --release -p reth -p reth-bench --features 'jemalloc asm-keccak rocksdb'"
    exit 1
fi

if [[ ! -x "$RETH_BENCH_BIN" ]]; then
    echo "Error: reth-bench binary not found at $RETH_BENCH_BIN"
    exit 1
fi

run_mdbx_benchmark() {
    local datadir="${HOODI_DATADIR_MDBX:-./hoodi-mdbx-bench}"
    
    echo ""
    echo "=== Running MDBX Benchmark ==="
    echo "Data directory: $datadir"
    
    # Unwind to start block
    echo "Unwinding to block $START_BLOCK..."
    "$RETH_BIN" stage unwind to-block "$START_BLOCK" \
        --datadir "$datadir" \
        --chain "$CHAIN" || true
    
    # Start reth in background
    echo "Starting reth (MDBX)..."
    "$RETH_BIN" node \
        --chain "$CHAIN" \
        --datadir "$datadir" \
        --metrics "localhost:$MDBX_METRICS_PORT" \
        --authrpc.port "$MDBX_AUTH_PORT" \
        --authrpc.jwtsecret "$JWT_SECRET" \
        --http \
        --http.port "$MDBX_HTTP_PORT" \
        --ws \
        --ws.port "$MDBX_WS_PORT" \
        2>&1 | tee "$RESULT_DIR/mdbx_reth.log" &
    RETH_PID=$!
    
    # Wait for reth to be ready
    echo "Waiting for reth to start..."
    sleep 10
    
    # Run benchmark
    echo "Running reth-bench..."
    "$RETH_BENCH_BIN" new-payload-fcu \
        --rpc-url "$RPC_URL" \
        --engine-rpc-url "http://localhost:$MDBX_AUTH_PORT" \
        --jwt-secret "$JWT_SECRET" \
        --from "$START_BLOCK" \
        --to "$END_BLOCK" \
        --output "$RESULT_DIR/mdbx_benchmark.csv" \
        --wait-for-persistence \
        2>&1 | tee "$RESULT_DIR/mdbx_bench.log"
    
    # Stop reth
    echo "Stopping reth..."
    kill "$RETH_PID" 2>/dev/null || true
    wait "$RETH_PID" 2>/dev/null || true
    
    echo "MDBX benchmark complete!"
}

run_rocks_benchmark() {
    local datadir="${HOODI_DATADIR_ROCKS:-./hoodi-rocks-bench}"
    
    echo ""
    echo "=== Running RocksDB Benchmark ==="
    echo "Data directory: $datadir"
    
    # Unwind to start block
    echo "Unwinding to block $START_BLOCK..."
    "$RETH_BIN" stage unwind to-block "$START_BLOCK" \
        --datadir "$datadir" \
        --chain "$CHAIN" || true
    
    # Start reth in background with RocksDB enabled
    echo "Starting reth (RocksDB)..."
    "$RETH_BIN" node \
        --chain "$CHAIN" \
        --datadir "$datadir" \
        --metrics "localhost:$ROCKS_METRICS_PORT" \
        --authrpc.port "$ROCKS_AUTH_PORT" \
        --authrpc.jwtsecret "$JWT_SECRET" \
        --http \
        --http.port "$ROCKS_HTTP_PORT" \
        --ws \
        --ws.port "$ROCKS_WS_PORT" \
        --storage.transaction-hash-numbers-in-rocksdb \
        --storage.accounts-history-in-rocksdb \
        --storage.storages-history-in-rocksdb \
        2>&1 | tee "$RESULT_DIR/rocks_reth.log" &
    RETH_PID=$!
    
    # Wait for reth to be ready
    echo "Waiting for reth to start..."
    sleep 10
    
    # Run benchmark
    echo "Running reth-bench..."
    "$RETH_BENCH_BIN" new-payload-fcu \
        --rpc-url "$RPC_URL" \
        --engine-rpc-url "http://localhost:$ROCKS_AUTH_PORT" \
        --jwt-secret "$JWT_SECRET" \
        --from "$START_BLOCK" \
        --to "$END_BLOCK" \
        --output "$RESULT_DIR/rocks_benchmark.csv" \
        --wait-for-persistence \
        2>&1 | tee "$RESULT_DIR/rocks_bench.log"
    
    # Stop reth
    echo "Stopping reth..."
    kill "$RETH_PID" 2>/dev/null || true
    wait "$RETH_PID" 2>/dev/null || true
    
    echo "RocksDB benchmark complete!"
}

generate_comparison() {
    echo ""
    echo "=== Generating Comparison Report ==="
    
    if [[ -f "$RESULT_DIR/mdbx_benchmark.csv" ]] && [[ -f "$RESULT_DIR/rocks_benchmark.csv" ]]; then
        # Simple comparison using awk
        echo ""
        echo "MDBX Results:"
        awk -F',' 'NR>1 {sum+=$2; count++} END {printf "  Avg latency: %.2f ms\n", sum/count}' "$RESULT_DIR/mdbx_benchmark.csv" 2>/dev/null || echo "  (No data)"
        
        echo ""
        echo "RocksDB Results:"
        awk -F',' 'NR>1 {sum+=$2; count++} END {printf "  Avg latency: %.2f ms\n", sum/count}' "$RESULT_DIR/rocks_benchmark.csv" 2>/dev/null || echo "  (No data)"
        
        echo ""
        echo "Full results saved to: $RESULT_DIR"
        echo ""
        echo "View in Grafana: https://reth-g.chiayong.com (Dashboard: RocksDB vs MDBX Benchmark)"
    fi
}

# Run benchmarks based on mode
case "$MODE" in
    mdbx)
        run_mdbx_benchmark
        ;;
    rocks)
        run_rocks_benchmark
        ;;
    both)
        run_mdbx_benchmark
        run_rocks_benchmark
        generate_comparison
        ;;
    *)
        usage
        ;;
esac

echo ""
echo "=== Benchmark Complete ==="
echo "Results saved to: $RESULT_DIR"
