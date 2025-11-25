# Iteration 103: Deep Dive into State Root Discrepancy

## Progress Summary

### What We've Verified ✅

1. **Genesis Block (Block 0)**: PERFECT MATCH
   - Hash: `0x77194da4...` ✅
   - State Root: `0x8647a2ae...` ✅
   - All fields match

2. **Block 1 Transactions**: PERFECT MATCH
   - All 3 transaction hashes match
   - Transaction 1: `0x1ac8d67d...` ✅
   - Transaction 2: `0x13cb79b0...` ✅
   - Transaction 3: `0x873c5ee3...` ✅

3. **Block 1 Execution**: CORRECT
   - Gas Used: `0x1d8a8` (120,008) ✅
   - Receipts Root: `0x171057d1...` ✅
   - Transactions Root: matches ✅

4. **Deposit Transaction Logic**: CORRECT
   - Official: `MintBalance(from, value)` + `Transfer(from, to, value)`
   - Ours: `mint_balance(from, value)` + `transfer_balance(from, to, value)`
   - ✅ Logic matches

5. **ArbOS Version**: CORRECT
   - Arbitrum Sepolia uses ArbOS version 6
   - Our config correctly specifies version 6
   - ✅ Configuration matches

### The Problem ❌

**Block 1 State Root Mismatch:**
- Our state root: `0x79b8b2a4...`
- Official state root: `0xd79377c8...`
- Block hash differs because it includes state root

### Analysis

Since:
- Genesis is correct
- Transactions match exactly
- Gas calculation matches
- Receipts match
- Transaction logic is correct

The issue MUST be in:
1. **ArbOS state updates** during block execution
2. **State trie calculation** (unlikely since receipts root works)
3. **Missing or incorrect state modifications**

### Suspects

#### Suspect #1: L1 Block Number Recording (HIGH PRIORITY)

We're seeing these logs repeatedly:
```
Skipping L1 block record: l1_block_number=4204685 <= old_l1_block_number=4204685
```

**Official Code** (`internal_tx.go:100-101`):
```go
if l1BlockNumber > oldL1BlockNumber {
    state.Restrict(state.Blockhashes().RecordNewL1Block(l1BlockNumber-1, prevHash, state.ArbOSVersion()))
}
```

**Our Code** (`execute.rs:537-547`):
```rust
if l1_block_number > old_l1_block_number {
    if let Err(e) = blockhashes.record_new_l1_block(
        l1_block_number - 1,
        prev_hash,
        arbos_version,
    ) {
        tracing::error!("Failed to record new L1 block: {:?}", e);
    }
}
```

**Question**: Are we reading `old_l1_block_number` correctly? If it's always equal to `l1_block_number`, we'd never record new L1 blocks, leading to missing state changes.

#### Suspect #2: Update Pricing Model Parameters

**Official Code** (`internal_tx.go:110`):
```go
state.L2PricingState().UpdatePricingModel(l2BaseFee, timePassed, false)
```

Note the `false` parameter!

**Our Code**: Need to check if we're passing the correct boolean parameter.

#### Suspect #3: Retryable Reaping

Both implementations try to reap 2 retryables, but we need to verify the reaping logic is correct.

#### Suspect #4: ArbOS Version Upgrade

**Official Code** (`internal_tx.go:112`):
```go
return state.UpgradeArbosVersionIfNecessary(currentTime, evm.StateDB, evm.ChainConfig())
```

Need to verify we're calling this correctly.

### Investigation Plan

#### Phase 1: Add Detailed State Change Logging

Add logging to track EXACTLY what state changes happen:

```rust
// In execute.rs, after each state modification:
reth_tracing::tracing::warn!(
    target: "arb-reth::STATE-DEBUG",
    "STATE_CHANGE: account={}, field={}, old={}, new={}",
    account, field, old_value, new_value
);
```

Target areas:
1. L1 block number storage
2. L2 pricing state
3. Retryable state
4. ArbOS version storage

#### Phase 2: Compare Storage Slots After Block 1

Query specific ArbOS storage slots after Block 1 execution:
- L1 block number slot
- L2 pricing state slots
- Compare with official chain

#### Phase 3: Binary Search Within Block Execution

If we can't find the issue, add logging at every step of block execution to pinpoint where state diverges:
1. After start block
2. After each transaction
3. After end block

### Key Questions

1. **Is `old_l1_block_number` being read correctly?**
   - If it's always equal to `l1_block_number`, L1 blocks never get recorded
   - This would cause state root mismatch

2. **Are we passing correct parameters to `UpdatePricingModel`?**
   - Official uses `false` as third parameter
   - Need to verify we match this

3. **Is retryable reaping working correctly?**
   - We call it twice like official code
   - But is the implementation correct?

4. **Are we upgrading ArbOS version correctly?**
   - This is called at end of start block
   - Could affect state if wrong

### Next Steps

1. ✅ Compare genesis - DONE
2. ✅ Compare transactions - DONE
3. ✅ Compare deposit logic - DONE
4. ⏳ Add state change logging - IN PROGRESS
5. ⏳ Check L1 block number reading
6. ⏳ Verify UpdatePricingModel parameters
7. ⏳ Compare storage slots after Block 1
8. ⏳ Fix the issue
9. ⏳ Verify all blocks match

### Confidence Level

**85%** - We've narrowed it down to ArbOS state management. The most likely culprit is either:
- L1 block number not being recorded (we're skipping it when we shouldn't)
- OR pricing model being updated incorrectly
- OR some other ArbOS storage not being updated

The fact that EVERYTHING else matches (transactions, gas, receipts) strongly suggests the bug is in a specific ArbOS state update, not in the core transaction execution logic.

---

Generated: 2025-11-25, Iteration 103
Status: 🎯 Narrowed to ArbOS state management
Next: Add state change logging and check L1 block number logic
Priority: CRITICAL - This is the actual bug
