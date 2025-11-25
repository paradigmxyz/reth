# Retry Transaction Hash Fix - Progress Report

## Problem Identified

Block 1 failed to match the official Arbitrum Sepolia chain due to incorrect retry transaction hash in the `RedeemScheduled` event.

### Root Causes Found

1. **Initial Issue**: Retry transaction hash was hardcoded as `B256::ZERO`
   - Location: `/home/dev/reth/crates/arbitrum/evm/src/execute.rs:988`
   - Impact: RedeemScheduled event had wrong topic[2], causing transaction to fail

2. **Second Issue**: Wrong `gas_fee_cap` value used in retry transaction
   - Used: Submit retryable's `maxFeePerGas` (0x3b9aca00)
   - Should use: Block's `baseFeePerGas` (0x5f5e100)
   - Impact: Incorrect retry tx hash computation

## Fixes Applied

### Fix 1: Implemented Retry Transaction Hash Computation
- Added `alloy-rlp` dependency to `reth-arbitrum-evm`
- Constructed `ArbRetryTx` structure with proper fields
- Encoded as `0x68 || RLP([12 fields])`
- Computed keccak256 hash of encoded bytes

**Commit**: `8b4b3d5bd` - "fix: compute correct retry transaction hash in submit retryable handler"

### Fix 2: Corrected gas_fee_cap Value
- Changed retry tx `gas_fee_cap` from `gas_fee_cap` variable to `ctx.basefee`
- This ensures retry transaction uses baseFeePerGas instead of maxFeePerGas

**Commit**: `9cd0e9f6e` - "fix: use baseFeePerGas for retry transaction gas_fee_cap"

## Expected Outcome

With these fixes, the retry transaction hash should now be:
```
0x873c5ee3092c40336006808e249293bf5f4cb3235077a74cac9cafa7cf73cb8b
```

This should result in:
1. ✅ Transaction status = 0x1 (success) instead of 0x0 (failed)
2. ✅ Correct RedeemScheduled event topic[2]
3. ✅ Correct effectiveGasPrice using baseFeePerGas
4. ✅ Matching receipts root
5. ✅ Matching state root
6. ✅ Matching block hash

## Testing Status

- Node restarted with fixes
- Waiting for first block production (~5 minutes)
- Will compare block 1 against official chain

## Next Steps

1. Verify block 1 matches exactly
2. Test blocks 2-100
3. Run full test suite for blocks 0-4000
