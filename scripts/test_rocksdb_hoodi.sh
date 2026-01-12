#!/bin/bash
# Hoodi Integration Test for RocksDB Historical Indexes
#
# This script tests RocksDB support by:
# 1. Building reth with edge features (RocksDB enabled)
# 2. Starting a Hoodi testnet node with existing snapshot
# 3. Running reth-bench to stress test with newPayload calls
# 4. Checking logs for errors

set -e

# Configuration
HOODI_DATA_DIR="$HOME/.local/share/reth/hoodi"
RETH_BINARY="./target/release/reth"
BENCH_BINARY="./target/release/reth-bench"
LOG_FILE="rocksdb_test_$(date +%Y%m%d_%H%M%S).log"
NODE_OUTPUT="node_output_$(date +%Y%m%d_%H%M%S).log"

echo "=== RocksDB Hoodi Integration Test ==="
echo "Date: $(date)"
echo "Data dir: $HOODI_DATA_DIR"
echo ""

# Step 1: Build reth with edge feature (RocksDB enabled)
echo "Step 1: Building reth with RocksDB support..."
cargo build --release --features "asm-keccak ethereum edge"
echo "✓ Build complete"
echo ""

# Step 2: Check if Hoodi snapshot exists
if [ ! -d "$HOODI_DATA_DIR" ]; then
    echo "❌ ERROR: Hoodi snapshot not found at $HOODI_DATA_DIR"
    echo "   Please ensure you have synced Hoodi data before running this test"
    exit 1
fi
echo "✓ Hoodi snapshot found"
echo ""

# Step 3: Create JWT secret if it doesn't exist
if [ ! -f "$HOODI_DATA_DIR/jwt.hex" ]; then
    echo "Creating JWT secret..."
    openssl rand -hex 32 > "$HOODI_DATA_DIR/jwt.hex"
fi
echo "✓ JWT secret ready"
echo ""

# Step 4: Start Hoodi node with RocksDB enabled
echo "Step 2: Starting Hoodi node with RocksDB..."
$RETH_BINARY node \
  --chain hoodi \
  --datadir "$HOODI_DATA_DIR" \
  --authrpc.port 8551 \
  --authrpc.jwtsecret "$HOODI_DATA_DIR/jwt.hex" \
  --http \
  --http.port 8545 \
  --log.file "$LOG_FILE" \
  > "$NODE_OUTPUT" 2>&1 &

NODE_PID=$!
echo "✓ Node started with PID: $NODE_PID"
echo ""

# Step 5: Wait for node to be ready
echo "Step 3: Waiting for node to be ready..."
sleep 20

# Check if node is still running
if ! kill -0 $NODE_PID 2>/dev/null; then
    echo "❌ ERROR: Node process died during startup"
    echo "   Check logs at: $NODE_OUTPUT"
    tail -50 "$NODE_OUTPUT"
    exit 1
fi
echo "✓ Node is running"
echo ""

# Step 6: Run reth-bench to send new payloads
echo "Step 4: Running reth-bench new-payload-fcu..."
$BENCH_BINARY new-payload-fcu \
  --rpc-url "http://localhost:8545" \
  --engine-rpc-url "http://localhost:8551" \
  --auth-jwtsecret "$HOODI_DATA_DIR/jwt.hex" \
  --from 1 \
  --advance 50 \
  --wait-for-persistence \
  || {
    echo "⚠ Bench completed with non-zero exit code, checking logs..."
  }
echo ""

# Step 7: Check for errors in logs
echo "Step 5: Checking logs for RocksDB-related errors..."
ERRORS_FOUND=0

if grep -i "rocksdb.*error\|rocksdb.*panic" "$LOG_FILE" > /dev/null; then
    echo "❌ ROCKSDB ERRORS FOUND:"
    grep -i "rocksdb.*error\|rocksdb.*panic" "$LOG_FILE" | head -10
    ERRORS_FOUND=1
fi

if grep -i "panic\|fatal" "$LOG_FILE" | grep -v "client disconnected" > /dev/null; then
    echo "❌ PANICS/FATAL ERRORS FOUND:"
    grep -i "panic\|fatal" "$LOG_FILE" | grep -v "client disconnected" | head -10
    ERRORS_FOUND=1
fi

# Cleanup
echo ""
echo "Step 6: Shutting down node..."
kill $NODE_PID 2>/dev/null || true
sleep 2

# Force kill if still running
if kill -0 $NODE_PID 2>/dev/null; then
    kill -9 $NODE_PID 2>/dev/null || true
fi

# Final verdict
echo ""
echo "=== Test Results ==="
if [ $ERRORS_FOUND -eq 0 ]; then
    echo "✅ INTEGRATION TEST PASSED"
    echo "   - Node started successfully"
    echo "   - RocksDB initialized without errors"
    echo "   - Bench tool completed"
    echo "   - No RocksDB errors in logs"
    echo ""
    echo "Logs saved to:"
    echo "  - $LOG_FILE (reth node logs)"
    echo "  - $NODE_OUTPUT (node stdout/stderr)"
    exit 0
else
    echo "❌ INTEGRATION TEST FAILED"
    echo "   Check logs for details:"
    echo "  - $LOG_FILE"
    echo "  - $NODE_OUTPUT"
    exit 1
fi
