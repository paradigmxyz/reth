# Next Steps for Receipt Type Fix

## Problem Identified
The RPC receipt converter in `crates/arbitrum/rpc/src/eth/receipt.rs` always wraps receipts in `ReceiptEnvelope::Legacy` at lines 91 and 161, causing all receipts to show type 0x0 in RPC responses.

## Root Cause
- `alloy_consensus::ReceiptEnvelope` only supports standard Ethereum types (0, 1, 2, 3, 4)
- Arbitrum types (0x64-0x6a) cannot be represented in this enum
- The converter needs to either:
  1. Use `WithOtherFields<TransactionReceipt>` to inject custom `type` field
  2. Create a custom Arbitrum receipt envelope type
  3. Modify `TransactionReceipt` serialization

## Attempted Fix
Changed line 91 to use correct envelope type for standard Ethereum types (0-3), but this is incomplete because:
- Arbitrum-specific types still can't be represented
- Line 161 also needs fixing
- May require return type change to `WithOtherFields<TransactionReceipt>`

## Recommended Approach
1. **Short-term**: Fix standard Ethereum types (0, 1, 2, 3) to use correct envelopes
2. **Medium-term**: Use `WithOtherFields` to inject correct type for Arbitrum types
3. **Long-term**: Add Arbitrum receipt envelope variants to arb-alloy

## Impact
Until this is fixed:
- Receipts root will be wrong (uses type byte in RLP encoding)
- Block hashes will never match
- RPC responses show incorrect type for all non-Legacy transactions

## Test Case
Block 3 has perfect gas match but wrong receipts root because:
- Tx 0: Should be type 0x6a (Internal), shows 0x0
- Tx 1: Should be type 0x69 (SubmitRetryable), shows 0x0
- Tx 2: Should be type 0x68 (Retry), shows 0x0

All other receipt fields match perfectly.

---
Status: Root cause identified, fix in progress
Priority: CRITICAL - blocks all progress on block matching
