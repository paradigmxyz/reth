# Status Update for Testing Agent - Iteration 49

## New Commits Applied ✅

The Testing Agent didn't detect these commits yet because the node wasn't rebuilt:

1. **Commit 3dbb195a6**: Initial cumulative gas fix attempt
2. **Commit 334c4f287**: Correct fix - check early_tx_gas before transaction type

## Node Status

- **Rebuilt**: ✅ Completed at ~04:56 UTC (3m 27s)
- **Database**: ✅ Cleaned
- **Node**: ✅ Started with PID 250756 at ~04:57 UTC
- **Blocks**: ⏳ Waiting for production (~5 minutes)

## What Changed (Commit 334c4f287)

### The Problem
Blocks 20, 35, 39-60 reported 0 gas because Internal transactions were using the wrong cumulative gas value from `ctx.cumulative_gas_used` instead of the stored `early_cumulative`.

### The Fix
Reordered the receipt builder logic to check `early_tx_gas` BEFORE checking transaction type:

```rust
// OLD (WRONG):
if ty == ArbTxType::Internal {
    (0, ctx.cumulative_gas_used)  // Wrong cumulative!
} else if let Some((gas, cum)) = get_early_tx_gas() {
    (gas, cum)  // Never reached for Internal!
}

// NEW (CORRECT):
if let Some((gas, cum)) = get_early_tx_gas() {
    (gas, cum)  // Correct for all early-terminated txs including Internal
} else if ty == ArbTxType::Internal {
    (0, ctx.cumulative_gas_used)  // Fallback
}
```

## Expected Results

### Block 20 (Main Test Case)
**Before Fix:**
- TX 0 (Internal): cumulative=0x54b4 (wrong) ❌
- TX 1: cumulative=0x0 (wrong) ❌
- Block gasUsed: 0x0 ❌

**After Fix (Expected):**
- TX 0 (Internal): cumulative=0x0 ✅
- TX 1: cumulative=0xc00e (49,166) ✅
- Block gasUsed: 0xc00e ✅

### All Blocks with Internal Transactions
- Should report correct gas usage
- Receipts root should match official chain
- State root will still differ (separate issue)

## Timeline

- **04:31 UTC**: Started build for commit 3dbb195a6
- **04:46 UTC**: Node started (discovered fix was incorrect)
- **04:51 UTC**: Analyzed issue, created correct fix (334c4f287)
- **04:53 UTC**: Pushed commit 334c4f287
- **04:56 UTC**: Rebuild completed
- **04:57 UTC**: Node restarted with clean database
- **~05:02 UTC**: Expected first blocks

## Testing Instructions

Wait until ~05:02 UTC, then check:

```bash
# Check block 20 gas
curl -s http://localhost:8547 -H "Content-Type: application/json" \
  -d '{"method":"eth_getBlockByNumber","params":["0x14",false],"id":1,"jsonrpc":"2.0"}' \
  | python3 -c "import sys,json; print('Block 20 gas:', json.load(sys.stdin)['result']['gasUsed'])"

# Should output: Block 20 gas: 0xc00e (49166)
```

## Remaining Issues

1. **State Root Mismatch**: PRIMARY blocker - all blocks differ
2. **Small Gas Discrepancies**: Block 2 has +6 gas
3. **Transaction Type Serialization**: TX 1 in block 20 shows as unknown type

---

Generated: 2025-11-25 04:57 UTC
Node: PID 250756 with commits 3dbb195a6 + 334c4f287
Status: Waiting for blocks (~5 min)
Branch: til/ai-fixes
