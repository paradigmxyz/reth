# Debugging Plan - Iteration 81

## Current Understanding

From manual testing (Iteration 79), we know:
1. ❌ Block 2: 6 gas discrepancy persists (0x10a29 vs 0x10a23)
2. ❌ Blocks 50+: Report 0 gas instead of non-zero
3. ⚠️ All transactions decode as Legacy type
4. 🔴 Receipts have cumulative_gas_used=0 when they shouldn't

## New Hypothesis: Wrong Transactions

When I queried official block 50 by number:
```
TX 1: type=0x2 (EIP-1559), gas=0xe79f
```

But when I queried by the TX hash from OUR block 50:
```
TX 1: type=0x64 (Deposit), gas=0x0
```

**This suggests**: Our block 50 TX 1 hash is DIFFERENT from official block 50 TX 1 hash!

If we're deriving wrong transactions from L1, then:
- ✅ Why all transactions are Legacy: Because the L1 messages we're reading contain different transactions
- ✅ Why gas is 0: Because the transactions we have are different types (Deposits vs EIP-1559)
- ✅ Why hashes don't match: Different transactions → different block hash

## Root Cause Possibilities

### Option A: Transaction Derivation is Wrong
- We're parsing L1 messages incorrectly
- We're creating wrong transaction lists
- StartBlock transactions are wrong or in wrong position
- SignedTx messages contain different data than expected

### Option B: Transaction Execution is Wrong
- We derive correct transactions
- But execution produces different results
- Receipts don't match official chain
- Gas accumulation is broken

### Option C: Both A and B
- We derive wrong transactions AND execute them wrong
- Multiple layers of issues

## Testing Strategy

### Step 1: Compare Transaction Lists
For blocks 1, 2, 50, verify:
1. Transaction count matches
2. Each transaction hash matches
3. Transaction order matches

If hashes don't match → Problem is in derivation (Option A)
If hashes match → Problem is in execution (Option B)

### Step 2: Compare Transaction Details
For each transaction that doesn't match:
1. Compare type, gas, from, to, value, input
2. Check if it's a StartBlock vs SignedTx vs other
3. Verify transaction encoding

### Step 3: Debug Transaction Derivation
If derivation is wrong:
1. Log L1 message data
2. Log parsed transaction data
3. Compare with Go implementation's parsing logic
4. Fix transaction derivation

### Step 4: Debug Receipt Building
If execution is wrong:
1. Enable full gas tracking logs
2. Trace cumulative gas through execution
3. Verify early_tx_gas storage/retrieval
4. Fix receipt cumulative gas

## Logging Improvements Needed

### For Transaction Comparison
```rust
// In node.rs where transactions are derived
tracing::info!(
    "Derived transactions for block",
    block = l2_block_number,
    tx_count = txs.len(),
    tx_hashes = ?txs.iter().map(|t| t.tx_hash()).collect::<Vec<_>>(),
);
```

### For Receipt Building
Already have good logging in:
- `set_early_tx_gas()` ✅
- `get_early_tx_gas()` ✅
- `clear_early_tx_gas()` ✅ (just added)
- Receipt builder paths ✅

### For Gas Accumulation
Need to add:
- Log cumulative_gas_used at start of each block
- Log cumulative_gas_used after each transaction
- Log final block gasUsed from last receipt

## Action Plan

1. ✅ Added logging to `clear_early_tx_gas` (Commit 15)
2. 🔄 Add transaction hash comparison logging
3. 🔄 Run test and compare transaction lists for blocks 1, 2, 50
4. 🔄 Based on results, either:
   - Fix transaction derivation, OR
   - Fix receipt/gas accumulation
5. 🔄 Verify fixes with new test run

## Expected Outcomes

### If Transaction Hashes Match
- Problem is in execution/receipts
- Focus on gas accumulation
- Fix early_tx_gas mechanism or receipt building

### If Transaction Hashes Don't Match
- Problem is in derivation
- Focus on L1 message parsing
- Compare with Go implementation's ParseL2Transactions

## Key Questions to Answer

1. Do our block transaction lists match official chain? (hashes)
2. Is early_tx_gas being stored correctly? (logs will show)
3. Is early_tx_gas being retrieved correctly? (logs will show)
4. Are receipts using the right cumulative source? (logs will show)
5. Are all transactions truly Legacy, or is decoding wrong?

---

Generated: 2025-11-25, Iteration 81
Status: Planning next debugging steps
Priority: Compare transaction lists first to isolate the problem
