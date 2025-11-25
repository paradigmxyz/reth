# CRITICAL BUG: Transaction Type Misidentification

## For Testing Agent - Iteration 53+

### Summary
The root cause of blocks reporting 0 gas is **NOT** a gas calculation bug. It's a **transaction type identification bug** that occurs before execution.

### Evidence

**Block 20 Example:**

Official Arbitrum Sepolia:
```
TX 0: Type 0x6a (Internal), cumulative_gas=0x0
TX 1: Type 0x2 (EIP-1559), cumulative_gas=0xc00e (49,166 gas)
Block gasUsed: 0xc00e
```

Our Node (with commits 334c4f287 + 3dbb195a6):
```
TX 0: Type Internal, cumulative_gas=0x0  ✅ Correct
TX 1: Type Deposit, cumulative_gas=0x0   ❌ WRONG TYPE!
Block gasUsed: 0x0                        ❌ Wrong
```

### Node Logs Confirm Bug

From node.log (PID 250756):
```
[2025-11-25T05:02:00] follower: executing tx tx_type=Deposit
  tx_hash=0x224081df0e185037a9b29fcec88c547e5977f784a7ed0c715b4c0b8ba2f66d43
```

But official chain shows this same hash as type 0x2 (EIP-1559).

### Why This Causes 0 Gas

When a transaction is misidentified as Deposit:
1. Deposit transactions may not contribute to cumulative gas
2. The receipt builder uses the wrong gas handling logic
3. Blocks with misidentified transactions report 0 gas

### Commits Made (Not Yet Tested by Testing Agent)

1. **3dbb195a6**: Attempted cumulative gas fix (incomplete - doesn't address root cause)
2. **334c4f287**: Fixed early_tx_gas priority (correct but insufficient)

These commits improve the gas calculation logic but DON'T fix the transaction type bug.

### Where The Bug Likely Is

The bug is in transaction **encoding/construction**, not decoding:

**File**: `/home/dev/reth/crates/arbitrum/primitives/src/lib.rs`
**Lines**: 1100-1159
**Function**: `Decodable2718::typed_decode`

This decode logic is CORRECT. It properly tries Arbitrum types first (0x64=Deposit), then Ethereum types (0x02=EIP1559).

The problem: The transaction bytes ALREADY have type byte 0x64 instead of 0x02 when they reach the decoder.

### Investigation Path

Need to trace backwards from decoding to find where type byte is set wrong:

1. `/home/dev/reth/crates/arbitrum/node/src/node.rs` line 523:
   ```rust
   let tx = ArbTransactionSigned::decode_2718(&mut s)?;
   ```
   The bytes in `s` have wrong type byte.

2. Where does `s` come from? From parsing L2MessageKind 0x04 (SignedTx).

3. Who creates/encodes these message bytes? Likely **nitro-rs message streamer**.

### Next Steps

1. **Add extensive logging** to show:
   - Raw message bytes from L1
   - First byte of transaction data (should be 0x02, is 0x64)
   - Where transaction bytes are constructed/encoded

2. **Compare with Go Nitro**:
   - `/home/dev/nitro/arbos/parse_l2.go` lines 159-174
   - How does Go handle SignedTx messages?

3. **Find the encoding bug** and fix it

### Impact

This bug blocks ALL progress:
- ❌ Can't match gas usage
- ❌ Can't match receipts root
- ❌ Can't match transactions root
- ❌ Can't match block hash
- ❌ State root also differs (separate issue)

### For Next Test Run

Please note that commits 334c4f287 and 3dbb195a6 exist but won't fix this issue. A new commit is needed to fix the transaction type encoding.

---

Generated: 2025-11-25 05:12 UTC
Status: ROOT CAUSE IDENTIFIED - Transaction type encoding bug
Priority: CRITICAL - Must fix before any other work
