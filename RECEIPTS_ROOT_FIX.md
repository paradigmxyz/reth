# Receipts Root Fix - Critical Bug Found

## The Bug

The receipts root was being calculated incorrectly because we were passing raw `ArbReceipt` objects directly to `calculate_receipt_root`, instead of wrapping them in `ReceiptWithBloom` first.

## Root Cause

According to EIP-2718 receipt encoding in Ethereum:
- Receipts in the receipts trie must be encoded **with their bloom filter**
- The `Encodable2718::encode_2718` method on `ReceiptWithBloom<R>` calls `R::eip2718_encode_with_bloom` which includes the bloom in the encoding
- Our direct `ArbReceipt::encode_2718` implementation encodes **without** bloom

## How Ethereum Does It

Looking at `reth/crates/ethereum/consensus/src/validation.rs`:

```rust
fn verify_receipts<R: Receipt>(
    expected_receipts_root: B256,
    expected_logs_bloom: Bloom,
    receipts: &[R],
) -> Result<(), ConsensusError> {
    // Wrap receipts in ReceiptWithBloom before calculating root!
    let receipts_with_bloom = receipts.iter().map(TxReceipt::with_bloom_ref).collect::<Vec<_>>();
    let receipts_root = calculate_receipt_root(&receipts_with_bloom);
    ...
}
```

## The Fix

**File**: `/home/dev/reth/crates/arbitrum/evm/src/config.rs`
**Location**: `assemble_block_inner` method, lines 58-61

**Before**:
```rust
let transactions_root = alloy_consensus::proofs::calculate_transaction_root(&input.transactions);
let receipts_root = alloy_consensus::proofs::calculate_receipt_root(&receipts);
let logs_bloom = alloy_primitives::logs_bloom(receipts.iter().flat_map(|r| r.logs()));
```

**After**:
```rust
let transactions_root = alloy_consensus::proofs::calculate_transaction_root(&input.transactions);
// Wrap receipts in ReceiptWithBloom before calculating root, as per EIP-2718 encoding
let receipts_with_bloom = receipts.iter().map(|r| r.with_bloom_ref()).collect::<Vec<_>>();
let receipts_root = alloy_consensus::proofs::calculate_receipt_root(&receipts_with_bloom);
let logs_bloom = alloy_primitives::logs_bloom(receipts.iter().flat_map(|r| r.logs()));
```

## Why This Was Hard to Find

1. **Receipt Content Was Correct**: All receipt fields (status, gasUsed, cumulativeGasUsed, logs) matched the official chain exactly
2. **Gas Calculations Were Correct**: Block gas usage matched perfectly in most blocks
3. **Type Bytes Were Present**: Our `encode_2718` implementation correctly prepended type bytes for non-legacy receipts
4. **RPC Layer Was Separate**: The RPC receipt display issue was a red herring - it only affected API responses, not consensus data

The issue was subtle: we were encoding receipts correctly for *display* but incorrectly for *the receipts trie*.

## Expected Impact

This fix should:
- ✅ Make receipts root match the official chain
- ✅ Make block hashes match the official chain
- ✅ Allow blocks 0-4000 to pass validation

## Commit

**Hash**: 9f93d523b
**Message**: fix(arbitrum): wrap receipts in ReceiptWithBloom before calculating receipts root

**Branch**: til/ai-fixes
**Pushed**: Yes

## Testing Status

- **Node**: Restarted with clean database (PID 237266)
- **Build**: Completed successfully at 2025-11-25 04:10:25 UTC
- **Database**: Cleaned before restart
- **Status**: Waiting for blocks to sync (~5 minutes)

## Next Steps

1. Wait for blocks to be produced (typically 5 minutes after startup)
2. Verify block 3 receipts root: should be `0x5b68dec16a1cab4377b435ba90db84513ced0bc973c80bf069f1f1cfd693dad6`
3. Verify block 1 hash: should be `0xa443906cf805a0a493f54b52ee2979c49b847e8c6e6eaf8766a45f4cf779c98b`
4. Run full test suite on blocks 0-4000

---
Generated: 2025-11-25 04:11 UTC
Node PID: 237266 (started 04:10:45 UTC)
