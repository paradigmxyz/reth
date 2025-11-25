# Critical Findings: Gas Reporting Issue - Iteration 69

## Problem Statement

**Requirement (req-1)**: All blocks up to 4000 (0xFA0) must match Arbitrum Sepolia exactly in transactions, gas usage, state root, and block hash.

**Current Status**:
- ✅ Blocks 1-12: Report correct gas values
- ❌ Blocks 13+: Report gasUsed = 0x0 (should be non-zero)

---

## Root Cause Analysis

### Finding 1: Transaction Type Mismatch

**Official Block 13**:
- tx 0: type 0x6a (Internal), gasUsed=0, cumulative=0x0
- tx 1: type 0x0 (Legacy), gasUsed=120875, cumulative=0x1d42b
- **Block gasUsed = 0x1d42b** ✅

**Our Block 13**:
- tx 0: type Internal, gasUsed=0, cumulative=0x0
- tx 1: type Internal, gasUsed=0, cumulative=0x0
- **Block gasUsed = 0x0** ❌

**Conclusion**: The second transaction should be Legacy, not Internal.

### Finding 2: Transaction Hash Mismatch

**Official Block 13 Transaction Hashes**:
- tx 0: `0xc229af29129edbf92a226eb6b4594354f7cc0644bb3ec952191e712de8296a26`
- tx 1: `0x803351deb6d745e91545a6a3e1c0ea3e9a6a02a1a4193b70edfcd2f40f71a01c`

**Our Block 13 Transaction Hashes**:
- tx 0: `0x94b16966687b3a82795ca6bd6a3cbaee93a58c024d91d59ae998769d83893a2e`
- tx 1: `0x15766445e81cd7c918f9daf5acd0537ccb2fcd52bf51ad8211a93748c3687566`

**Conclusion**: Our node is creating COMPLETELY DIFFERENT transactions than the official chain!

### Finding 3: Blocks 1-12 Have Correct Gas

Blocks 1-12 report correct gas values matching the official chain. This suggests:
1. Early blocks (1-12) either have only non-Internal transactions, OR
2. Early blocks have correct transaction types that contribute to gas

This indicates the issue starts at a specific point (block 13), not from genesis.

---

## Technical Analysis

### How Cumulative Gas Works

Code location: `/home/dev/reth/crates/arbitrum/evm/src/build.rs`

```rust
let gas_to_add = if is_internal { 0u64 } else { actual_gas };
let new_cumulative = self.cumulative_gas_used + gas_to_add;
```

- **Internal transactions**: Don't contribute to cumulative gas (gas_to_add = 0)
- **Other transactions**: Add their gas to cumulative (gas_to_add = actual_gas)

**For a block to report non-zero gas**:
- At least one transaction must be non-Internal type
- That transaction must execute successfully
- The last receipt's cumulative_gas_used becomes block's gasUsed

### Why Block 13 Reports Zero Gas

Block 13 has 2 transactions, both typed as Internal:
1. tx 0 (Internal): cumulative = 0 + 0 = 0
2. tx 1 (Internal): cumulative = 0 + 0 = 0
3. **Block gasUsed = last cumulative = 0**

If tx 1 were correctly typed as Legacy:
1. tx 0 (Internal): cumulative = 0 + 0 = 0
2. tx 1 (Legacy): cumulative = 0 + 120875 = 120875
3. **Block gasUsed = last cumulative = 120875** ✅

---

## Deeper Issue: Transaction Data Mismatch

The transaction hash mismatch reveals a **fundamental synchronization problem**:

1. **Transactions don't match**: Our node creates different transactions than official chain
2. **This affects everything**:
   - Transaction hashes differ
   - Receipt roots differ
   - State roots differ
   - Block hashes differ

3. **Potential causes**:
   - Reading wrong L1 batch data
   - Decoding batch data incorrectly
   - ArbOS state divergence
   - Different transaction ordering
   - Missing delayed inbox messages

---

## Investigation Path Forward

### Question 1: Where Do Transactions Come From?

**For blocks with SignedTx messages (0x04)**:
- Transactions come from L1 batch data
- Decoded in `/home/dev/reth/crates/arbitrum/node/src/node.rs`
- Fixed by commit a6f99fe12 (Legacy RLP handling)

**For blocks with only StartBlock (0x00)**:
- Transactions might come from:
  1. **ArbOS delayed inbox** - Transactions submitted to L1 delayed inbox
  2. **ArbOS retryable tickets** - L1→L2 messages being retried
  3. **ArbOS internal operations** - System transactions

### Question 2: Why Do Hashes Differ?

Transaction hashes are cryptographic hashes of transaction data. If hashes differ, the transactions themselves are fundamentally different:
- Different sender address
- Different nonce
- Different gas parameters
- Different data
- Different signature

This suggests we're not reading the same source data as the official chain.

### Question 3: How to Fix This?

**Option A: Fix Transaction Source**
- Identify where block 13's transactions should come from
- Verify we're reading that source correctly
- Compare official vs our implementation

**Option B: Compare L1 Data**
- Check what L1 batch contains block 13
- Verify we're reading the same L1 batch as official chain
- Ensure batch data decoding matches

**Option C: Check ArbOS State**
- ArbOS maintains delayed inbox, retryable tickets
- Verify ArbOS state matches official chain
- Check if transactions are pulled from ArbOS correctly

---

## Recommended Next Steps

### Immediate Action

1. **Stop current debugging approach** - Transaction type is a symptom, not root cause
2. **Identify transaction source** - Where should block 13 txs come from?
3. **Compare with official Go implementation** - How does Nitro handle this?

### Specific Tasks

1. Check if block 13 has delayed inbox messages
2. Check if block 13 has retryable tickets being executed
3. Verify L1 batch data for blocks around 13
4. Compare our ArbOS state with official chain at block 12

### Long-term Fix

The gas reporting issue is a symptom of transaction data mismatch. Fixing gas alone won't solve req-1 because:
- Transaction hashes won't match
- State roots won't match
- Block hashes won't match

We need to fix the **root cause**: Why are we creating different transactions than the official chain?

---

## Summary

**Initial Problem**: Blocks 13+ report 0 gas

**Root Cause Found**: Transactions typed incorrectly (all Internal instead of mixed types)

**Deeper Issue**: Transaction data completely mismatches official chain (different hashes)

**Real Problem**: Not syncing the same transaction data as official Arbitrum Sepolia

**Next Step**: Identify where block 13's transactions originate and why we have different data

---

Generated: 2025-11-25 06:35 UTC
Priority: CRITICAL
Status: Requires fundamental investigation of transaction data source
Branch: til/ai-fixes

**Recommendation**: Before attempting any more fixes, we need to understand WHY our transactions differ from the official chain. This is a data synchronization issue, not just a type classification issue.
