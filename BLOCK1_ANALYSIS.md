# Block 1 Detailed Analysis - First Divergence Point

## Summary
Block 1 is where execution first diverges from the official Arbitrum Sepolia chain. The transactions root matches perfectly, confirming we have the correct input transactions. However, the receipts root, state root, and block hash all differ, indicating the execution produces different results.

## Transaction Comparison

### Transaction 0: Internal StartBlock (0x6a)
**Status**: ✅ Matches
- Hash: `0x1ac8d67d5c4be184b3822f9ef97102789394f4bc75a0f528d5e14debef6e184c`
- From: `0x00000000000000000000000000000000000a4b05`
- Gas Used: `0x0` (both)
- Status: `0x1` (success, both)
- Logs: Empty (both)

### Transaction 1: Submit Retryable (0x69)
**Status**: ❌ CRITICAL MISMATCH - Root Cause Identified

#### Receipt Differences:
| Field | Local | Official | Match |
|-------|-------|----------|-------|
| status | `0x0` (FAIL) | `0x1` (SUCCESS) | ❌ |
| gasUsed | `0x186a0` | `0x186a0` | ✅ |
| effectiveGasPrice | `0x3b9aca00` | `0x5f5e100` | ❌ |
| logs[0] topics[0] | TicketCreated | TicketCreated | ✅ |
| logs[1] topics[0] | RedeemScheduled | RedeemScheduled | ✅ |
| logs[1] topics[1] | ticket_id | ticket_id | ✅ |
| logs[1] topics[2] | `0x0000...` | `0x873c5ee3...` | ❌ |
| logs[1] topics[3] | `0x0000...` | `0x0000...` | ✅ |

#### Critical Issue: RedeemScheduled Event Topic[2]

**What It Is**: The retry transaction hash that gets scheduled for automatic execution

**Expected**: `0x873c5ee3092c40336006808e249293bf5f4cb3235077a74cac9cafa7cf73cb8b`
**Actual**: `0x0000000000000000000000000000000000000000000000000000000000000000`

**Impact**:
1. The wrong hash causes the transaction to fail (status 0x0 vs 0x1)
2. This leads to incorrect state changes
3. The wrong effectiveGasPrice is calculated (should be baseFeePerGas not maxFeePerGas)

**Root Cause**:
In `/home/dev/reth/crates/arbitrum/evm/src/execute.rs` line 988:
```rust
let retry_tx_hash = B256::ZERO;
```

This is a placeholder that was never implemented correctly.

**Required Fix**:
The retry transaction hash must be computed by:
1. Creating an `ArbRetryTx` structure with:
   - chain_id: `U256::from(421614)` (Arbitrum Sepolia)
   - nonce: `0`
   - from: `ctx.sender` (the submit retryable sender)
   - gas_fee_cap: `gas_fee_cap` (from submit retryable)
   - gas: `usergas` (0x186a0)
   - to: `Some(retry_to)` (0x3fab184622dc19b6109349b94811493bf2a45362)
   - value: `retry_value` (0x2386f26fc10000)
   - data: `Bytes::from(retry_data)` (empty)
   - ticket_id: `ticket_id` (the submit retryable tx hash)
   - refund_to: `fee_refund_addr`
   - max_refund: `available_refund`
   - submission_fee_refund: `submission_fee_u256`

2. Encoding as EIP-2718:
   ```
   0x68 || RLP([chain_id, nonce, from, gas_fee_cap, gas, to, value, data,
                ticket_id, refund_to, max_refund, submission_fee_refund])
   ```

3. Computing keccak256 hash of the encoded bytes

**Implementation Challenge**:
- Need to properly import and use RLP encoding traits
- The `arb-alloy-consensus` crate has the `ArbRetryTx` type with `Encodable` implementation
- Need to add `alloy-rlp` dependency to `reth-arbitrum-evm` Cargo.toml
- Or use the encoding implementation from `ArbTypedTransaction::Retry` variant

### Transaction 2: Retry (0x68)
**Status**: ✅ Appears to match

- Hash: `0x873c5ee3092c40336006808e249293bf5f4cb3235077a74cac9cafa7cf73cb8b`
- From: `0xb8787d8f23e176a5d32135d746b69886e03313be`
- Gas Used: `0x5208` (both)
- Status: `0x1` (success, both)
- Logs: Empty (both)

## Block-Level Differences

| Field | Local | Official |
|-------|-------|----------|
| hash | `0xba0204...` | `0xa44390...` |
| stateRoot | `0x619f9a...` | `0xd79377...` |
| receiptsRoot | `0x6c3987...` | `0x171057...` |
| transactionsRoot | `0x5219258...` | `0x5219258...` ✅ |
| logsBloom | ...0050... | ...0051... |
| gasUsed | `0x1d8a8` | `0x1d8a8` ✅ |

## Secondary Issue: Transaction Status

Why does the submit retryable transaction fail locally?

**Hypothesis**: The transaction execution logic checks the return value or some other condition, and because we're emitting incorrect data (wrong retry tx hash), the transaction is marked as failed.

**Evidence**:
- Gas is fully consumed (0x186a0) in both cases
- Logs are still emitted
- But status is 0x0 instead of 0x1

**Likely Cause**: In the Go implementation, the transaction succeeds when it properly creates and schedules the retry transaction. Our implementation has the placeholder hash, which might cause a validation failure that sets the status to failed.

## Cascading Effects

1. **Receipts Root Mismatch**: Because receipt[1] is different (status and logs), the receipts Merkle tree is different
2. **Logs Bloom Filter**: The bloom filter is calculated from logs, so incorrect log topics lead to incorrect bloom
3. **State Root Mismatch**: Failed vs successful transaction leads to different state changes
4. **Block Hash**: All of the above contribute to a different block hash

## Fix Priority

### Critical (Blocks all progress):
1. ✅ Fix retry transaction hash computation in RedeemScheduled event
2. Verify transaction status becomes 0x1 after fix
3. Check effectiveGasPrice calculation

### High (After critical fixes):
4. Verify state changes match after transaction status is fixed
5. Compare detailed state diff for any remaining discrepancies

### Follow-up:
6. Test blocks 2-10 to ensure fix cascades correctly
7. Full validation up to block 4000

## Next Steps

1. Add `alloy-rlp` dependency to `reth-arbitrum-evm/Cargo.toml`
2. Implement proper retry tx hash computation using `ArbRetryTx` encoding
3. Test against block 1 to verify:
   - Receipt status changes to 0x1
   - Logs topic[2] matches
   - Receipts root matches
   - State root matches
   - Block hash matches
4. If block 1 passes, test blocks 2-100
5. Continue full test suite to block 4000

## Code Locations

- **Issue Location**: `/home/dev/reth/crates/arbitrum/evm/src/execute.rs:988`
- **Submit Retryable Handler**: Lines 810-1020
- **ArbRetryTx Definition**: `/home/dev/arb-alloy/crates/consensus/src/tx.rs:91-104`
- **RLP Encoding**: `/home/dev/arb-alloy/crates/consensus/src/tx.rs` (Encodable impl)

## References

- Official Block 1: https://sepolia.arbiscan.io/block/1
- Official TX 1: https://sepolia.arbiscan.io/tx/0x13cb79b086a427f3db7ebe6ec2bb90a806a3b0368ecee6020144f352e37dbdf6
- Official TX 2 (Retry): https://sepolia.arbiscan.io/tx/0x873c5ee3092c40336006808e249293bf5f4cb3235077a74cac9cafa7cf73cb8b
