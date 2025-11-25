# CRITICAL FINDING: Receipt Type Mismatch Root Cause

## Discovery
Block 3 has PERFECT gas match (both local and official: 0x1d8a8), yet receipts root and block hash are completely different.

## Root Cause Identified
Receipt types are being stored correctly in the receipt builder, but the RPC layer returns type 0x0 (Legacy) for ALL transaction types.

### Evidence from Block 3:
```
Tx 0 (Internal):
  - Local RPC type: 0x0 (WRONG - should be 0x6a)
  - Official type: 0x6a ✓

Tx 1 (SubmitRetryable):
  - Local RPC type: 0x0 (WRONG - should be 0x69)
  - Official type: 0x69 ✓

Tx 2 (Retry):
  - Local RPC type: 0x0 (WRONG - should be 0x68)
  - Official type: 0x68 ✓
```

All other receipt fields match perfectly (status, gas, cumulative gas, logs count).

## Technical Analysis

### What Works ✓
1. Receipt builder creates correct ArbReceipt variant:
   - `ArbReceipt::Internal(receipt)` for type 0x6a
   - `ArbReceipt::SubmitRetryable(receipt)` for type 0x69
   - `ArbReceipt::Retry(receipt)` for type 0x68

2. EIP-2718 encoding calls `self.ty()` which returns correct type byte

3. Gas calculation is mostly correct (some blocks match perfectly)

### What's Broken ❌
**RPC Serialization**: The type information is lost between storage and RPC response.

When `ArbReceipt` is serialized for RPC, the serde default serialization includes the variant name but not the type byte. The RPC layer needs to:
1. Detect the receipt variant
2. Extract the type using `receipt.ty()` method
3. Include it in the JSON response

### Impact on Block Hash
Receipts root is calculated from RLP-encoded receipts WITH type bytes.

If receipts are stored as Legacy (type 0x0) instead of their actual types, the receipts root will be wrong, causing block hash mismatch.

## Critical Question
Are receipts STORED with wrong type, or just DISPLAYED with wrong type in RPC?

To determine this, we need to:
1. Check raw receipt bytes from storage
2. Verify if RLP encoding includes correct type byte
3. Trace through RPC serialization pipeline

## Fix Required
The RPC layer (likely in `crates/arbitrum/rpc` or `crates/rpc`) needs modification to:

1. **For Receipt Responses**:
   - Extract type byte using `receipt.ty()` or `Typed2718::ty()`
   - Include in JSON as `"type": "0x6a"` etc.

2. **Verify Storage Encoding**:
   - Confirm receipts are stored with correct EIP-2718 encoding
   - If not, receipt builder needs fixing

## Hypothesis
Based on the EIP-2718 fix calling `self.ty()`, receipts are likely STORED correctly but RPC serialization loses the type information.

## Next Steps (Priority Order)
1. **URGENT**: Verify receipt storage encoding (check raw bytes)
2. **URGENT**: Fix RPC receipt serialization to include correct type
3. Test with fresh DB to verify receipts root matches
4. Address remaining small gas discrepancies (4-6 gas in some blocks)

## Expected Outcome
Once RPC type is fixed AND receipts are stored with correct types:
- Receipts root should match official chain
- Block hashes should match (if state roots also match)
- All blocks should pass verification

---
Timestamp: 2025-11-25 03:42 UTC
Status: CRITICAL BUG IDENTIFIED - FIX IN PROGRESS
