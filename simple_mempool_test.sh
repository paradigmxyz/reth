#!/bin/bash

# Simplified test that works with dev mode auto-mining

echo "=== MEMPOOL CLEAR TEST (Simplified) ==="

# 1. Check initial state
echo -e "\n1. Initial mempool status:"
curl -s -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"txpool_status","params":[],"id":1}' | jq '.result'

# 2. Get current nonce
NONCE=$(cast nonce 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266 --rpc-url http://localhost:8545)
echo -e "\n2. Current nonce: $NONCE"

# 3. Send transactions with gaps (these will be queued)
echo -e "\n3. Sending transactions with nonce gaps..."
echo "   Sending tx with nonce $(($NONCE + 2)) (gap - will be queued)"
TX1=$(cast send --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --nonce $(($NONCE + 2)) \
  0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7 --value 0.001ether \
  --async 2>/dev/null)
echo "   TX1: ${TX1:0:10}..."

echo "   Sending tx with nonce $(($NONCE + 3)) (gap - will be queued)"
TX2=$(cast send --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --nonce $(($NONCE + 3)) \
  0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7 --value 0.001ether \
  --async 2>/dev/null)
echo "   TX2: ${TX2:0:10}..."

sleep 1

# 4. Check mempool (should have queued transactions)
echo -e "\n4. Mempool after sending (should have queued txs):"
MEMPOOL_BEFORE=$(curl -s -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"txpool_status","params":[],"id":1}' | jq '.result')
echo "$MEMPOOL_BEFORE"

QUEUED_BEFORE=$(echo $MEMPOOL_BEFORE | jq -r '.queued' | xargs printf "%d")
if [ "$QUEUED_BEFORE" -gt 0 ]; then
    echo "   ✓ Successfully created $QUEUED_BEFORE queued transactions"
else
    echo "   ⚠ No queued transactions created - test may not work properly"
fi

# 5. Trigger block production by sending a valid transaction
echo -e "\n5. Triggering block production..."
echo "   Sending valid tx with nonce $NONCE to trigger mining"
TRIGGER_TX=$(cast send --rpc-url http://localhost:8545 \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --nonce $NONCE \
  0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb7 --value 0.001ether 2>&1)

if echo "$TRIGGER_TX" | grep -q "blockNumber"; then
    BLOCK=$(echo "$TRIGGER_TX" | grep blockNumber | awk '{print $2}')
    echo "   ✓ Transaction mined in block $BLOCK"
else
    echo "   Transaction sent, waiting for mining..."
fi

sleep 2

# 6. Final check
echo -e "\n6. FINAL MEMPOOL STATUS:"
MEMPOOL_AFTER=$(curl -s -X POST http://localhost:8545 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","method":"txpool_status","params":[],"id":1}' | jq '.result')
echo "$MEMPOOL_AFTER"

PENDING_AFTER=$(echo $MEMPOOL_AFTER | jq -r '.pending' | xargs printf "%d")
QUEUED_AFTER=$(echo $MEMPOOL_AFTER | jq -r '.queued' | xargs printf "%d")

echo -e "\n=== TEST RESULT ==="
if [ "$QUEUED_BEFORE" -gt 0 ] && [ "$PENDING_AFTER" -eq 0 ] && [ "$QUEUED_AFTER" -eq 0 ]; then
    echo "✅ SUCCESS! Mempool was completely cleared!"
    echo "   Before: $QUEUED_BEFORE queued transactions"
    echo "   After:  0 transactions (all cleared)"
    echo ""
    echo "The --txpool.clear-on-canonical-state-change flag is working perfectly!"
    echo "All transactions (including queued ones) were removed when the block became canonical."
else
    echo "❌ Test did not demonstrate clearing as expected"
    echo "   Before: Queued=$QUEUED_BEFORE"
    echo "   After:  Pending=$PENDING_AFTER, Queued=$QUEUED_AFTER"
    
    if [ "$QUEUED_BEFORE" -eq 0 ]; then
        echo "   Issue: Could not create queued transactions to test with"
    elif [ "$QUEUED_AFTER" -gt 0 ]; then
        echo "   Issue: Queued transactions were not cleared"
    fi
fi