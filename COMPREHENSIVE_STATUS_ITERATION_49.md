# Comprehensive Status - Iteration 49

## Commits Made

1. **3dbb195a6**: Initial attempt to fix cumulative gas for Internal transactions
   - Changed receipts.rs to preserve ctx.cumulative_gas_used for Internal txs
   - ❌ INCORRECT - didn't account for early_tx_gas priority

2. **334c4f287**: Fixed order - check early_tx_gas BEFORE transaction type
   - Reordered if-else to check early_tx_gas first
   - ✅ CORRECT logic order
   - ❌ BUT doesn't fix the root issue

## Current Status

Node rebuilt and restarted with both fixes. Testing revealed a **DEEPER** issue:

### Block 20 Analysis

**Official Chain:**
- TX 0: Type 0x6a (Internal), cumulative=0x0
- TX 1: Type 0x2 (EIP-1559), cumulative=0xc00e (49,166 gas)
- Block gasUsed: 0xc00e

**Our Node:**
- TX 0: Type Internal ✅, cumulative=0x0 ✅
- TX 1: Type **Deposit** ❌, cumulative=0x0 ❌
- Block gasUsed: 0x0 ❌

## Root Cause: Transaction Type Misidentification

The second transaction in block 20 is being identified as **Deposit** instead of **EIP-1559**.

### Evidence from Logs

```
[2025-11-25T05:02:00] follower: executing tx tx_type=Deposit
  tx_hash=0x224081df0e185037a9b29fcec88c547e5977f784a7ed0c715b4c0b8ba2f66d43
[2025-11-25T05:02:00] finalized_tx_types=["Internal", "Deposit"]
```

### How Go Nitro Works

From `/home/dev/nitro/arbos/parse_l2.go`:

When message type is `L1MessageType_L2FundedByL1`:
1. Parse the `kind` byte from msg.L2msg[0]
2. Call `parseUnsignedTx()` to create the actual transaction
3. Create a **Deposit transaction** wrapper
4. Return BOTH: `[deposit, actual_tx]`

So `L2FundedByL1` creates TWO transactions:
- A Deposit (for funding)
- The actual transaction (Unsigned, Contract, etc.)

### The Problem

Our Rust implementation is either:
1. **Not parsing the message correctly** - Misidentifying the transaction type from the L2 message bytes
2. **Handling L2FundedByL1 incorrectly** - Perhaps only creating the Deposit, not the second transaction
3. **Transaction type detection bug** - The second transaction IS being created but with wrong type

### Why This Matters

Deposit transactions have different gas handling:
- They may not contribute to cumulative gas (need to verify)
- They have different receipt fields
- They serialize differently in RPC

This explains:
- ✅ Why blocks 20, 35, 39-60 report 0 gas
- ✅ Why receipt types don't match
- ✅ Why RPC shows missing fields (no "type", "gas", "to")

## Investigation Needed

### 1. Check Message Decoding
Where in our codebase do we:
- Decode L1 messages into transactions
- Handle `L2FundedByL1` message type
- Parse the transaction type byte

**Likely locations:**
- `/home/dev/nitro-rs/crates/inbox/` - Message parsing
- `/home/dev/arb-alloy/` - Transaction type definitions
- `/home/dev/reth/crates/arbitrum/` - Transaction execution

### 2. Compare Transaction Encoding

Both chains have the same transaction hash (0x224081df...), which means the transaction DATA is identical. But we're interpreting the TYPE differently.

**Hypothesis**: The message contains a `SignedTx` (L2MessageKind = 4) which should be parsed as an EIP-1559 transaction. But we're somehow creating a Deposit instead.

### 3. Trace the Flow

Need to add logging to show:
1. What message type we receive
2. What L2 message kind byte we read
3. How we determine the transaction type
4. What transaction struct we create

## Next Steps

### Priority 1: Find Transaction Type Bug

Search for where we handle `L2FundedByL1` or `SignedTx` messages:

```bash
# In nitro-rs
grep -r "L2FundedByL1\|SignedTx\|parse.*message" /home/dev/nitro-rs/crates/

# In arb-alloy
grep -r "transaction.*type\|ArbTxType" /home/dev/arb-alloy/

# In reth
grep -r "L2.*message\|parse.*transaction" /home/dev/reth/crates/arbitrum/
```

### Priority 2: Understand Gas Handling for Deposits

Do Deposit transactions contribute to cumulative gas or not?
- Check official Go code
- Check our executor logic for Deposit vs other types

### Priority 3: Fix the Parser

Once we find where the bug is:
1. Fix transaction type detection
2. Ensure correct transaction creation
3. Commit with clear message
4. Rebuild and test

## Current Node State

- **PID**: 250756
- **Blocks**: Synced to ~4298
- **Commits**: 3dbb195a6 + 334c4f287 applied
- **Status**: Running but producing wrong transaction types

## Impact on Requirements

**req-1**: BLOCKED by transaction type bug
- Can't match gas usage (wrong tx types)
- Can't match receipts root (wrong receipt types)
- Can't match block hash (wrong transactions root)
- State root also differs (separate issue)

---

Generated: 2025-11-25 05:08 UTC
Node: PID 250756
Status: CRITICAL BUG FOUND - Transaction type misidentification
Priority: Fix transaction parsing before any other work
Branch: til/ai-fixes
