# Submit Retryable Transaction Status Fix - Summary

## Problem
Submit retryable transactions (type 0x69) were failing with status=0x0 instead of status=0x1.

## Root Cause Analysis

### Investigation Process
1. Added detailed logging to `receipts.rs` to capture ExecutionResult
2. Discovered ALL submit retryable transactions fail with `InvalidFEOpcode` (HaltReason)
3. Traced execution flow:
   - Submit retryable transaction calls address 0x6e (ARB_RETRYABLE_TX predeploy)
   - `start_tx` hook processes the transaction completely and returns `end_tx_now=true, error=None`
   - Predeploy dispatch is skipped when `end_tx_now=true` (line 343 in build.rs)
   - Transaction still executes through EVM at address 0x6e
   - Address 0x6e has stub code `0xFE` (INVALID opcode) from genesis
   - EVM hits INVALID opcode and halts with `InvalidFEOpcode`

### Why This Happened
Predeploy addresses (0x64-0x70) are initialized with stub code `0xFE` to mark them as occupied and prevent direct execution. They are meant to be handled exclusively by the predeploy dispatch mechanism or completely skipped.

When `start_tx` hook returns `end_tx_now=true` with `error=None`, it indicates the transaction has been fully processed within the hook itself (emitting logs, creating retryables, scheduling retry transactions, etc.). These transactions should NOT execute through the EVM at all.

## Solution Implemented

### File: `/home/dev/reth/crates/arbitrum/evm/src/build.rs` (lines 454-520)

Added logic to skip EVM execution entirely for early-terminated successful transactions:

```rust
// For early-terminated successful transactions, skip EVM execution
// The transaction has already been fully processed in start_tx hook
let result = if start_hook_result.end_tx_now && start_hook_result.error.is_none() {
    tracing::info!(
        target: "arb-reth::executor",
        tx_hash = ?tx_hash,
        gas_used = start_hook_result.gas_used,
        "Skipping EVM execution for early-terminated successful transaction"
    );

    // Store gas for receipt
    let gas_to_add = if is_internal { 0u64 } else { start_hook_result.gas_used };
    let new_cumulative = self.cumulative_gas_used + gas_to_add;
    let gas_for_receipt = if is_internal { 0u64 } else { start_hook_result.gas_used };
    crate::set_early_tx_gas(tx_hash, gas_for_receipt, new_cumulative);

    tracing::debug!(
        target: "arb-reth::executor",
        tx_hash = ?tx_hash,
        gas_used = start_hook_result.gas_used,
        gas_to_add = gas_to_add,
        new_cumulative = new_cumulative,
        "Stored gas for early-terminated transaction"
    );

    // Return success with gas used (will create receipt with success status)
    Ok(Some(start_hook_result.gas_used))
} else {
    // For normal transactions or early-terminated errors, execute through EVM
    self.inner.execute_transaction_with_commit_condition(wrapped, |exec_result| {
        // ... normal execution flow
    })
};
```

### Key Aspects of the Fix

1. **Conditional Check**: `start_hook_result.end_tx_now && start_hook_result.error.is_none()`
   - Only applies to transactions fully processed in the hook
   - Only when processing succeeded (no error)

2. **Gas Tracking**: Preserves gas information for receipt builder
   - Stores gas in `early_tx_gas` map for later retrieval
   - Respects internal transaction gas handling (internal txs don't add to cumulative)

3. **Return Value**: `Ok(Some(gas_used))`
   - Signals successful execution
   - Receipt builder will create receipt with status=0x1
   - Gas value is available for receipt

4. **Backward Compatibility**: Normal transactions and early-terminated errors still execute through EVM as before

## Expected Impact

### Before Fix
- Submit retryable transactions: status=0x0 (failed), reason=InvalidFEOpcode
- Block receipts root: Mismatched due to wrong status
- Block state root: Mismatched (failed tx doesn't apply state changes)
- Block hash: Mismatched (depends on receipts root and state root)

### After Fix
- Submit retryable transactions: status=0x1 (success)
- Retry tx hash: Already fixed to match (0x873c5e...)
- Block receipts root: Should match official chain
- Block state root: Should match official chain
- Block hash: Should match official chain

## Build Status
✅ Successfully compiled with `cargo build --release -p arb-reth`

Completion time: 2m 58s
Warnings: Non-critical (unused imports, variables)

## Testing Status
⏳ Awaiting Testing Agent verification

The fix has been implemented and compiled. The next test iteration will verify whether:
1. Submit retryable transactions now have status=0x1
2. Block 1 receipts root matches official chain
3. Block 1 state root matches official chain
4. Block 1 hash matches official chain

## Related Files Modified

1. `/home/dev/reth/crates/arbitrum/evm/src/build.rs` - Main fix implementation
2. `/home/dev/reth/crates/arbitrum/evm/src/receipts.rs` - Added diagnostic logging (can be removed after verification)
3. `/home/dev/reth/crates/arbitrum/evm/src/execute.rs` - Previous retry tx hash fix
4. `/home/dev/reth/crates/arbitrum/evm/Cargo.toml` - Added alloy-rlp dependency

## Commit Recommendation

```
fix(arbitrum): skip EVM execution for early-terminated successful transactions

Submit retryable transactions (type 0x69) were failing with InvalidFEOpcode
because they were executing in the EVM after being fully processed in the
start_tx hook. Predeploy addresses have stub code (0xFE) to prevent direct
execution.

When start_tx returns end_tx_now=true with error=None, the transaction has
been completely processed in the hook and should not execute through EVM.

This fix adds logic to skip EVM execution for such transactions, returning
success directly with the gas used from the hook. This prevents hitting the
INVALID opcode while ensuring receipts are created with correct status=0x1.

Fixes block 1 submit retryable transaction failures.
```

## Next Steps

1. ✅ Wait for Testing Agent to run next iteration
2. ⏳ Verify block 1 now passes with matching hashes
3. ⏳ If block 1 passes, verify blocks 2-100 pass
4. ⏳ If blocks 2-100 pass, verify all blocks up to 4000
5. ⏳ Once validation complete, commit and push changes
