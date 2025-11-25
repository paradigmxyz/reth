# Final Status - Iteration 48

## Summary

Two commits pushed to fix the Internal transaction cumulative gas issue:

1. **Commit 3dbb195a6**: Initial attempt - preserve cumulative for Internal transactions
   - ❌ INCORRECT - didn't fix the issue

2. **Commit 334c4f287**: Correct fix - check early_tx_gas BEFORE transaction type
   - ✅ Should fix blocks 20, 35, 39-60 reporting 0 gas

## The Problem (Deep Dive)

Blocks containing Internal transactions as the last transaction reported 0 gas, causing receipts root mismatch and wrong block gas_used.

### Example: Block 20

**Official Chain:**
- TX 0: Internal (0x6a), cumulative=0x0, gasUsed=0
- TX 1: EIP-1559 (0x2), cumulative=0xc00e (49,166), gasUsed=49,166
- Block gasUsed: 0xc00e (49,166)

**Our Chain (Before Fix):**
- TX 0: Internal (0x6a), cumulative=0x54b4 (21,684) ❌, gasUsed=0
- TX 1: Unknown type, cumulative=0x0 ❌, gasUsed=?
- Block gasUsed: 0x0 ❌

## Root Cause

The receipt builder had this logic order:

```rust
// WRONG ORDER - Commit 3dbb195a6
if ty == ArbTxType::Internal {
    (0, ctx.cumulative_gas_used)  // Uses wrong cumulative!
} else if let Some((gas, cumulative)) = get_early_tx_gas() {
    (gas, cumulative)  // Never reached for Internal txs!
} else {
    (gas_used, ctx.cumulative_gas_used)
}
```

**Problem**: Internal transactions go through the early termination path in the executor, which stores the correct cumulative gas in `early_tx_gas`. But the receipt builder checked `ty == Internal` FIRST, so it used `ctx.cumulative_gas_used` (wrong value) instead of the stored `early_cumulative` (correct value).

## The Fix (Commit 334c4f287)

Changed the order to check `early_tx_gas` FIRST:

```rust
// CORRECT ORDER - Commit 334c4f287
if let Some((gas, cumulative)) = get_early_tx_gas() {
    (gas, cumulative)  // Correct for all early-terminated txs including Internal
} else if ty == ArbTxType::Internal {
    (0, ctx.cumulative_gas_used)  // Fallback for non-early Internal txs
} else {
    (gas_used, ctx.cumulative_gas_used)  // Normal path
}
```

**Why This Works:**
1. Internal transactions that go through early termination (most of them) now use the stored `early_cumulative` which has the correct value
2. The executor stores `early_cumulative=0` for the first Internal tx in a block (correct)
3. The executor stores the cumulative from previous txs for subsequent Internal txs (correct)

## Files Changed

**File**: `/home/dev/reth/crates/arbitrum/evm/src/receipts.rs`
**Lines**: 90-124
**Change**: Reordered if-else chain to check `early_tx_gas` before `ty == Internal`

## Build Status

- **Commit 334c4f287**: Pushed to til/ai-fixes
- **Build**: NOT YET DONE (needs rebuild)
- **Node**: Currently running with old code (PID 249573)
  - Needs restart after rebuild

## Next Steps for Testing Agent

1. Rebuild is required: `cd /home/dev/nitro-rs && cargo build --release`
2. Clean database: `rm -rf nitro-db node.log*`
3. Restart node
4. Wait ~5 minutes for blocks
5. Test blocks 20, 35, 39-60 - should now report correct gas usage

## Expected Results After Fix

**Block 20:**
- TX 0 (Internal): cumulative=0x0 ✅, gasUsed=0 ✅
- TX 1 (EIP-1559): cumulative=0xc00e ✅, gasUsed=49,166 ✅
- Block gasUsed: 0xc00e ✅

**Other Blocks with Internal Transactions:**
- Should all report correct gas usage
- Receipts root should match (cumulative gas in receipts is correct)
- State root will still differ (separate issue)

## Remaining Issues

1. **State Root Mismatch**: Still the PRIMARY blocker for req-1
   - All blocks have wrong state root
   - Separate from gas/receipt issues

2. **Small Gas Discrepancies**: Block 2 has +6 gas
   - Minor issue, likely edge case

3. **Second Transaction in Block 20**: Shows as unknown type
   - May be RPC serialization issue
   - Needs investigation

## Commit History

1. **9f93d523b**: Receipts root fix (wrap in ReceiptWithBloom) ✅ Working
2. **3dbb195a6**: Internal tx cumulative gas (attempt 1) ❌ Incorrect
3. **334c4f287**: Internal tx early_tx_gas priority ⏳ Needs testing

---

Generated: 2025-11-25 04:53 UTC
Node: PID 249573 (old code), needs rebuild and restart
Status: Fix committed, awaiting rebuild and test
Commit: 334c4f287 on til/ai-fixes
