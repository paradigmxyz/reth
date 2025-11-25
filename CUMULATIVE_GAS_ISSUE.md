# Cumulative Gas Issue - Analysis

## Problem

Blocks with Internal transactions still report 0 gas after the fix in commit 3dbb195a6.

## Example: Block 20

### Official Chain
- TX 0: Internal (0x6a), receipt cumulative=0x0
- TX 1: EIP-1559 (0x2), receipt cumulative=0xc00e (49,166)
- Block gasUsed: 0xc00e (49,166)

### Our Chain (After Fix)
- TX 0: Internal (0x6a), receipt cumulative=0x54b4 (21,684) ❌ WRONG
- TX 1: Unknown type, receipt cumulative=0x0 ❌ WRONG
- Block gasUsed: 0x0 ❌ WRONG

## Root Cause

The fix in `receipts.rs` line 101 changed Internal transactions to:
```rust
(0u64, ctx.cumulative_gas_used)  // Preserve cumulative
```

This is WRONG because:

1. **For the FIRST transaction in a block**, ctx.cumulative_gas_used is NOT 0 - it's whatever the EVM gas was for that transaction (21,684 in this case)

2. **The executor** is setting the ctx.cumulative_gas_used based on the EVM gas consumed, NOT on the cumulative from previous transactions

## The Real Issue

Looking at the logs:
```
STORING early_tx_gas tx_hash=0x5f7cebba... gas_used=0 cumulative_gas=0
```

and

```
Created receipt with cumulative_gas_used tx_hash=0x5f7cebba... cumulative_in_receipt=21684
```

This shows that:
1. The executor stores cumulative=0 via `set_early_tx_gas`
2. But the receipt builder uses ctx.cumulative_gas_used=21684

So the executor's `set_early_tx_gas` is being called with cumulative=0, but then the receipt builder is using a DIFFERENT cumulative value from ctx!

## Investigation Needed

1. Why is ctx.cumulative_gas_used = 21684 for the first transaction when it should be 0?

2. Is the executor's `self.cumulative_gas_used` not being reset between blocks?

3. Should Internal transactions use the early_tx_gas path at all, or should they go through the normal path?

## Next Steps

Need to understand the relationship between:
- `self.cumulative_gas_used` in the executor
- `ctx.cumulative_gas_used` passed to receipt builder
- `set_early_tx_gas` and `get_early_tx_gas`

The executor seems to be calculating cumulative gas incorrectly, or the receipt builder is not respecting the stored early_tx_gas for Internal transactions.

---

Generated: 2025-11-25 04:52 UTC
Node: PID 249573, Block 2947
Status: Investigating cumulative gas calculation
