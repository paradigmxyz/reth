# Root Cause Found - Iteration 69

## Executive Summary

**Problem**: Blocks 13+ report gasUsed = 0x0 (should be non-zero)

**Root Cause**: ArbOS-created transactions are all typed as "Internal" (0x6a), when some should be Legacy/EIP1559/etc based on their actual transaction data.

**Impact**: Internal transactions don't contribute to cumulative gas, so when a block has only Internal transactions, its gasUsed is 0.

---

## Discovery Process

### Initial Hypothesis
Blocks 20+ report 0 gas, blocks 1-19 correct.

### Investigation
1. **Checked BlockExecutionResult**: gas_used = 0 at the source
2. **Checked build.rs gas correction**: Working correctly when receipts have correct cumulative gas
3. **Checked receipt cumulative gas**: Last receipt has cumulative_gas_used = 0
4. **Checked early_tx_gas storage**: Both tx in block 13 stored with cumulative=0
5. **Checked transaction types**: BOTH tx typed as "Internal" (0x6a)

### Key Finding
Official Arbitrum Sepolia block 13:
- tx 0: type 0x6a (Internal), gasUsed=0, cumulative=0x0
- tx 1: type 0x0 (Legacy), gasUsed=120875, cumulative=0x1d42b
- **Block gasUsed = 0x1d42b** ✅

Our node block 13:
- tx 0: type Internal, gasUsed=0, cumulative=0
- tx 1: type Internal, gasUsed=0, cumulative=0
- **Block gasUsed = 0x0** ❌

**Conclusion**: The second transaction should be type Legacy (0x0), not Internal (0x6a).

---

## Technical Analysis

### How Cumulative Gas Works

In `/home/dev/reth/crates/arbitrum/evm/src/build.rs`:

```rust
let gas_to_add = if is_internal { 0u64 } else { actual_gas };
let new_cumulative = self.cumulative_gas_used + gas_to_add;
crate::set_early_tx_gas(tx_hash, gas_for_receipt, new_cumulative);
```

- Internal transactions: `gas_to_add = 0` → cumulative doesn't increase
- Other transactions: `gas_to_add = actual_gas` → cumulative increases

For block 13:
- tx 0 (should be Internal): cumulative = 0 + 0 = 0 ✅
- tx 1 (should be Legacy): cumulative = 0 + 120875 = 120875 ✅

But because tx 1 is incorrectly marked as Internal:
- tx 1 (incorrectly Internal): cumulative = 0 + 0 = 0 ❌

### Where Transaction Type Comes From

Block 13 has only a StartBlock message (0x00), no SignedTx messages (0x04). This means the transactions are created by ArbOS hooks during `start_tx`.

The key question: **How does ArbOS communicate the actual transaction type (Legacy, EIP1559, etc.) for transactions it creates?**

Possible locations:
1. **StartBlock message data**: May contain transaction data
2. **ArbOS state**: May store pending transactions
3. **Hook return value**: start_tx hook might return transaction data

---

## Hypothesis: StartBlock Contains Transaction Data

Looking at block 13 StartBlock:
```
start_data_len=132, start_data_prefix=[6b, f6, a4, 2d]
```

132 bytes is enough for transaction data. The official Go node must be decoding this data to extract:
1. Transaction type (Legacy, EIP1559, etc.)
2. Transaction fields (nonce, gasPrice, to, value, data, etc.)
3. Signature (v, r, s)

Our code might be:
- Decoding the StartBlock data correctly
- But wrapping it in an Internal transaction type
- Instead of determining the actual type from the transaction data

---

## Where to Look for Fix

### Option 1: Check StartBlock Decoding
File: Likely in nitro-rs inbox message handling

Look for: Where StartBlock messages are converted to transactions

Expected: Should decode transaction type from data, not always use Internal

### Option 2: Check ArbOS Hook
File: `/home/dev/reth/crates/arbitrum/evm/src/hooks.rs` or similar

Look for: `start_tx` hook implementation

Expected: Should extract transaction type from ArbOS state or StartBlock data

### Option 3: Check Transaction Creation
File: Likely in arbos precompile code

Look for: Where transactions are assembled from Start Block data

Expected: Should preserve original transaction type, not force Internal

---

## Next Steps

1. ✅ **Identified**: Transactions from StartBlock are all typed as Internal
2. ⏳ **Find**: Where StartBlock data is decoded into transactions
3. ⏳ **Fix**: Extract actual transaction type from data
4. ⏳ **Test**: Verify blocks 13+ report correct gas

---

## Log Evidence

### Block 13 Transaction Types (Our Node)
```
TX Result: Success tx_hash=0x94b... tx_type=Internal gas=21672
TX Result: Success tx_hash=0x1576... tx_type=Internal gas=22028
```

### Block 13 Cumulative Gas (Our Node)
```
STORING early_tx_gas tx_hash=0x94b... gas_used=0 cumulative_gas=0
STORING early_tx_gas tx_hash=0x1576... gas_used=0 cumulative_gas=0
```

### Block 13 Official Chain
```
Block 13: gasUsed=0x1d42b, txs=2
  tx 0: type=0x6a (Internal), gas=0x0
  tx 1: type=0x0 (Legacy), gas=0x3c4d8

receipt 0: cumulativeGasUsed=0x0, gasUsed=0x0
receipt 1: cumulativeGasUsed=0x1d42b, gasUsed=0x1d42b
```

---

Generated: 2025-11-25 06:25 UTC
Priority: CRITICAL - This is the root cause of req-1 blocking issue
Branch: til/ai-fixes
Next: Find StartBlock transaction decoding logic
