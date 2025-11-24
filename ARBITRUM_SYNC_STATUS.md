# Arbitrum Sepolia Sync Status

## Current Status

The Rust implementation of Arbitrum Nitro can now:
- ✅ Build successfully
- ✅ Start and sync from L1
- ✅ Produce blocks (tested up to block 4298)
- ❌ Match official Arbitrum Sepolia blocks exactly

## Test Results (Blocks 1-10)

**Result**: 10/10 blocks have mismatches

### Critical Issues

1. **Transaction Count Mismatch**
   - Blocks 4 and 8 have 2 transactions locally vs 1 transaction officially
   - Extra transaction example (Block 4):
     - Hash: `0x803351deb6d745e91545a6a3e1c0ea3e9a6a02a1a4193b70edfcd2f40f71a01c`
     - From: `0xbb6e024b9cffacb947a71991e386681b1cd1477d`
     - Has invalid signature values: `r: 0x247000`, `s: 0x2470`
   - This appears to be an incorrectly scheduled retryable transaction

2. **Gas Usage Differences**
   - Most blocks show 4-6 gas difference
   - Block 2: local `0x10a29` vs official `0x10a23` (6 gas difference)
   - Many blocks show error: "total gas used < poster gas component, gasUsed=0 posterGas=0"
   - This suggests issues in poster gas calculation logic

3. **State Root Mismatches**
   - All blocks have different state roots
   - Cascading effect from transaction and gas differences
   - Example Block 1:
     - Local: `0x619f9ae5c567458c77efcdd20dd6105c1332651c283f517808b54d2ad4dc5d9e`
     - Official: `0xd79377c8506f4e7bdb8c3f69f438ca4e311addcd6f53c62a81a978ea86d9612b`

4. **Receipts Root Mismatches**
   - All blocks have different receipts roots
   - Related to state and gas calculation differences

5. **Logs Bloom Differences**
   - Subtle differences in bloom filters
   - Block 1: local has `0050` while official has `0051`

## Code Fixes Applied

### 1. Receipt Converter Type Fix
**File**: `/home/dev/reth/crates/arbitrum/rpc/src/eth/receipt.rs`

**Issue**: Type mismatch - `ArbReceiptConverter::RpcReceipt` was returning `TransactionReceipt<ReceiptEnvelope>` but should return plain `TransactionReceipt`.

**Fix**:
- Changed `RpcReceipt` type to `TransactionReceipt`
- Added proper conversion from `TransactionReceipt<ReceiptEnvelope>` to `TransactionReceipt`
- Saved `next_log_index` before moving `input` to avoid borrow issues
- Properly converted consensus logs to RPC logs

**Commit**: `8590e180c`

## Architecture Overview

### Transaction Flow
1. **Inbox Reader** (nitro-rs) → Reads L1 messages
2. **Message Streamer** (nitro-rs) → Streams messages to execution
3. **Block Builder** (reth) → Builds blocks with transactions
4. **ArbOS Hooks** (reth) → Processes Arbitrum-specific logic
5. **EVM Execution** (reth) → Executes transactions

### Key Components

#### ArbOS State (`crates/arbitrum/evm/src/arbosstate.rs`)
- Manages ArbOS version and state
- Handles L1/L2 pricing
- Manages retryables and address tables

#### ArbOS Hooks (`crates/arbitrum/evm/src/execute.rs`)
- `start_tx`: Pre-execution hook for special transaction types (0x64 deposit, 0x6a internal, 0x68 retry, 0x69 submit retryable)
- `gas_charging`: L1 calldata cost calculation and poster gas
- `end_tx`: Post-execution cleanup and refunds
- `scheduled_txes`: Should return scheduled retryable transactions (currently returns empty vec)

## Investigation Needed

### 1. Extra Transaction Source
**Location**: Likely in block builder or transaction pool
**Question**: Where are the extra transactions (like the one in block 4) being added?
**Hypothesis**: Something is scheduling retryable transactions incorrectly

### 2. Poster Gas Calculation
**Location**: `crates/arbitrum/evm/src/execute.rs:1038-1062`
**Error**: "total gas used < poster gas component, gasUsed=0 posterGas=0"
**Question**: Why is this occurring for internal transactions (0x6a type)?
**Hypothesis**: Internal transactions should have 0 gas, but the error handling might be incorrect

### 3. Scheduled Transactions
**Location**: `crates/arbitrum/evm/src/execute.rs:1264-1293`
**Current**: Returns empty Vec after finding `RedeemScheduled` events
**Question**: When should scheduled transactions actually be added to blocks?
**Action**: Compare with Go implementation in `/home/dev/nitro/arbos/tx_processor.go`

### 4. Gas Accounting Differences
**Question**: What causes the 4-6 gas differences in most blocks?
**Hypothesis**: Could be:
- Intrinsic gas calculation differences
- L1 calldata cost calculation differences
- Poster gas calculation differences
- EVM gas metering differences

## Go Implementation Reference

Key files to study in `/home/dev/nitro`:
- `arbos/tx_processor.go` - Main transaction processing logic
- `arbos/block_processor.go` - Block-level processing
- `arbos/retryables/retryable.go` - Retryable transaction handling
- `arbos/l1pricing/l1pricing.go` - L1 cost calculations
- `arbos/internal_tx.go` - Internal transaction handling

## Next Steps

### Immediate (Critical Path)
1. **Find and remove extra transaction source**
   - Debug block builder to see where transactions are added
   - Check if transactions are coming from message streamer
   - Verify retryable scheduling logic

2. **Fix poster gas error**
   - Understand when posterGas should be 0
   - Fix the error condition in EndTxHook
   - Ensure internal transactions (0x6a) handle gas correctly

3. **Fix gas calculation differences**
   - Compare L1 calldata cost calculation with Go implementation
   - Verify intrinsic gas calculations
   - Check base fee and gas price calculations

### Medium Term
4. **Implement scheduled transactions correctly**
   - Study Go implementation of `ScheduledTxes()`
   - Understand when retryables should be auto-redeemed
   - Implement proper retryable transaction creation from logs

5. **Verify receipts generation**
   - Ensure receipt fields are calculated correctly
   - Verify gas used in receipts
   - Check logs bloom calculation

### Validation
6. **Test against official chain**
   - Compare blocks 1-100
   - Compare blocks 1000-1100
   - Compare blocks 3900-4000
   - Ensure all fields match exactly

## Running Tests

### Build
```bash
cd /home/dev/nitro-rs
cargo build --release
```

### Clean Start
```bash
rm -rf nitro-db node.log*
```

### Run Node
```bash
nohup ./target/release/arb-nitro-rs --network arbitrum-sepolia \
  --l1-rpc-url https://eth-sepolia.g.alchemy.com/v2/SZtN3OkrMwNmsXx5x-4Db \
  --beacon-url https://eth-sepoliabeacon.g.alchemy.com/v2/SZtN3OkrMwNmsXx5x-4Db \
  --chain-id 421614 > node.log 2>&1 &
```

Wait ~5 minutes for blocks to start being produced.

### Check Block Number
```bash
curl http://localhost:8547 -X POST -H "Content-Type: application/json" \
  --data '{"method":"eth_blockNumber","params":[],"id":1,"jsonrpc":"2.0"}'
```

### Compare Blocks
```bash
python3 compare_blocks.py 1 100
```

### Check Specific Block
```bash
# Local
curl http://localhost:8547 -X POST -H "Content-Type: application/json" \
  --data '{"method":"eth_getBlockByNumber","params":["0x1",true],"id":1,"jsonrpc":"2.0"}' | python3 -m json.tool

# Official
curl https://arb-sepolia.g.alchemy.com/v2/SZtN3OkrMwNmsXx5x-4Db -X POST \
  -H "Content-Type: application/json" \
  --data '{"method":"eth_getBlockByNumber","params":["0x1",true],"id":1,"jsonrpc":"2.0"}' | python3 -m json.tool
```

## Conclusion

The implementation is close but needs critical fixes to match the official chain exactly. The main issues are:
1. Extra transactions being added (blocks 4, 8)
2. Small gas calculation differences (4-6 gas)
3. Cascading state/receipts differences

With focused debugging on the transaction source and gas calculations, these issues should be resolvable. The ArbOS infrastructure is in place and mostly working correctly.
