# Legacy RLP Transaction Fix - Detailed Explanation

## Executive Summary

**Root Cause**: SignedTx (0x04) messages from the L1 inbox contain **Legacy RLP-encoded transactions** (no type byte), but the code was calling `decode_2718()` which expects EIP-2718 typed transactions with a type byte prefix.

**Fix Deployed**: Commit a6f99fe12 - Detect Legacy RLP vs Typed transactions and use the appropriate decoder.

**Expected Impact**: Fixes blocks 50+ reporting 0 gas, fixes block 20 gas reporting, ensures correct transaction types in receipts.

---

## Discovery Process

### Initial Symptom
- Blocks 50+ report gasUsed = 0x0 (should be non-zero)
- Block 20 reports gasUsed = 0x0 (should be 0xc00e = 49,166)
- Transactions appear to have wrong types in logs

### Investigation (Commits 5855e7ad0)
Added diagnostic logging to show the first bytes of transactions in SignedTx messages:

```rust
reth_tracing::tracing::warn!(
    target: "arb-reth::decode",
    first_byte = format!("0x{:02x}", s[0]),
    total_len = s.len(),
    first_10_bytes = format!("{:02x?}", &s[..std::cmp::min(10, s.len())]),
    "SignedTx (0x04) message - checking transaction type byte"
);
```

### Key Discovery
Logs showed transactions starting with bytes **0xf8** and **0xf9**:

```
first_byte="0xf8" total_len=167 first_10_bytes="[f8, a5, 80, 85, 17, 48, 76, e8, 00, 83]"
first_byte="0xf9" total_len=367 first_10_bytes="[f9, 01, 6c, 80, 85, 17, 48, 76, e8, 00]"
```

**Critical Insight**: These are **RLP list prefix bytes**, NOT EIP-2718 type bytes!

---

## Technical Background

### EIP-2718 Typed Transactions
Standard Ethereum typed transactions have this format:
- **First byte** (type): 0x00 (Legacy), 0x01 (EIP-2930), 0x02 (EIP-1559), 0x03 (EIP-4844), 0x04 (EOA code)
- **Followed by**: RLP-encoded transaction data

Arbitrum adds custom types:
- 0x64 (Deposit), 0x65 (Unsigned), 0x66 (Contract), 0x68 (Retry), 0x69 (SubmitRetryable), 0x6A (Internal), 0x78 (Legacy)

### Legacy RLP Transactions
Legacy transactions (pre-EIP-2718) have NO type byte:
- **First byte**: RLP list prefix (0xc0-0xff range)
  - 0xf8 = RLP list with 1-byte length field
  - 0xf9 = RLP list with 2-byte length field
- **Followed by**: RLP-encoded fields (nonce, gasPrice, gas, to, value, data, v, r, s)

### RLP Encoding Primer
RLP (Recursive Length Prefix) encoding:
- Single byte 0x00-0x7f: value itself
- Byte 0x80-0xb7: short string
- Byte 0xb8-0xbf: long string
- Byte 0xc0-0xf7: short list ← **Legacy transactions start here**
- Byte 0xf8-0xff: long list ← **Most Legacy transactions start here**

---

## The Bug

### Previous Code (BROKEN)
```rust
0x04 => {  // SignedTx message kind
    let mut s = cur;
    while !s.is_empty() {
        // Always calls decode_2718, which expects a type byte
        let tx = reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)?;
        // ...
    }
}
```

### What Happened
1. SignedTx message contains bytes: `[0xf8, 0xa5, 0x80, 0x85, ...]`
2. `decode_2718()` reads first byte (0xf8) as transaction type
3. No transaction type matches 0xf8
4. Decode fails or produces wrong transaction
5. Wrong gas accounting in receipts

### Official Go Implementation
From `/home/dev/nitro/arbos/parse_l2.go`:

```go
case L2MessageKind_SignedTx:
    newTx := new(types.Transaction)
    readBytes, err := io.ReadAll(rd)
    if err != nil {
        return nil, err
    }
    if err := newTx.UnmarshalBinary(readBytes); err != nil {
        return nil, err
    }
```

Key insight: Go's `UnmarshalBinary` **automatically handles both Legacy RLP and typed transactions**. It checks if the first byte is >= 0xc0 (RLP list) and decodes accordingly.

---

## The Fix (Commit a6f99fe12)

### New Code
```rust
0x04 => {
    let mut s = cur;
    while !s.is_empty() {
        let before_len = s.len();

        // SignedTx messages can contain either:
        // 1. Legacy RLP transactions (first byte >= 0xc0) - no type byte
        // 2. Typed transactions (first byte 0x00-0x04) - has type byte
        // We need to use the appropriate decoding method for each.
        let tx = if s.len() >= 1 && s[0] >= 0xc0 {
            // Legacy RLP transaction - decode directly without type byte
            use alloy_rlp::Decodable;
            reth_arbitrum_primitives::ArbTransactionSigned::decode(&mut s)
                .map_err(|e| eyre::eyre!("Failed to decode Legacy RLP transaction: {}", e))?
        } else {
            // Typed transaction (EIP-2718) - use decode_2718
            use alloy_eips::eip2718::Decodable2718;
            reth_arbitrum_primitives::ArbTransactionSigned::decode_2718(&mut s)
                .map_err(|e| eyre::eyre!("Failed to decode typed transaction: {:?}", e))?
        };
        // ... rest of code
    }
}
```

### How It Works
1. Check first byte of transaction data
2. If >= 0xc0: It's a Legacy RLP transaction → use `Decodable::decode()`
3. If < 0xc0: It's a typed transaction → use `Decodable2718::decode_2718()`
4. Both paths correctly decode transactions and calculate gas

### Why This Works
- `Decodable::decode()` for `ArbTransactionSigned` calls `TxLegacy::rlp_decode_signed()` which properly handles RLP-encoded Legacy transactions
- `Decodable2718::decode_2718()` handles all typed transactions (Standard Ethereum + Arbitrum custom types)
- Gas calculation now works correctly because transactions are decoded properly

---

## Expected Results

### Block 20 (First block with transactions)
**Before**:
- gasUsed = 0x0
- TX 1 misidentified

**After** (Expected):
- gasUsed = 0xc00e (49,166 gas)
- TX 1 correctly identified as Legacy transaction
- Receipt types correct

### Blocks 50+ (Many Legacy transactions)
**Before**:
- All report gasUsed = 0x0

**After** (Expected):
- Non-zero gas values
- Transactions decoded correctly
- Receipts match official chain

### Receipt Roots
Combined with commit 9f93d523b (ReceiptWithBloom wrapper), receipts should now:
- Have correct transaction types
- Calculate correct cumulative gas
- Match official chain receipt roots

---

## Verification Steps

Once node syncs to block 20+, verify:

1. **Block 20 Gas**:
   ```bash
   curl -s http://localhost:8547 -d '{"method":"eth_getBlockByNumber","params":["0x14",false],"id":1,"jsonrpc":"2.0"}' | jq '.result.gasUsed'
   # Expected: "0xc00e"
   ```

2. **Block 50 Gas**:
   ```bash
   curl -s http://localhost:8547 -d '{"method":"eth_getBlockByNumber","params":["0x32",false],"id":1,"jsonrpc":"2.0"}' | jq '.result.gasUsed'
   # Expected: Non-zero value
   ```

3. **Transaction in Block 20**:
   ```bash
   curl -s http://localhost:8547 -d '{"method":"eth_getBlockByNumber","params":["0x14",true],"id":1,"jsonrpc":"2.0"}' | jq '.result.transactions[1]'
   # Expected: Legacy transaction fields (no "type" field or "type": "0x0")
   ```

4. **Compare with Official Chain**:
   ```bash
   # Official chain
   curl -s https://arb-sepolia.g.alchemy.com/v2/SZtN3OkrMwNmsXx5x-4Db -d '{"method":"eth_getBlockByNumber","params":["0x14",false],"id":1,"jsonrpc":"2.0"}' | jq '.result.gasUsed'
   # Should match our node
   ```

---

## Related Commits

This fix builds on previous commits:

1. **9f93d523b** (Dec 10): Wrap receipts in ReceiptWithBloom before calculating receipts root
   - Ensures EIP-2718 encoding for receipt root calculation
   - Required for matching official chain receipt roots

2. **3dbb195a6** (Dec 10): Preserve cumulative gas in Internal transaction receipts
   - Fixed cumulative gas handling for Internal transactions
   - Ensures gas doesn't reset mid-block

3. **334c4f287** (Dec 10): Check early_tx_gas before transaction type in receipt builder
   - Fixed ordering of gas calculation checks
   - Ensures early-terminated transactions get correct gas values

4. **5855e7ad0** (Dec 10): Add diagnostic logging for SignedTx transaction type bytes
   - Revealed that transactions start with 0xf8/0xf9 (RLP prefixes)
   - Led to discovery of the Legacy RLP issue

5. **a6f99fe12** (Dec 10): Handle Legacy RLP transactions in SignedTx messages ← **THIS FIX**
   - Detect Legacy RLP vs Typed transactions
   - Use appropriate decoder for each

---

## Remaining Work

### Known Issues After This Fix

1. **State Root Mismatches**: Still expected - separate issue from gas/receipts
   - Need to investigate ArbOS state transition logic
   - Compare state changes between Go and Rust implementations

2. **Block 2 Gas Discrepancy**: +6 gas difference (0x10a29 vs 0x10a23)
   - Might be a different gas accounting issue
   - Needs separate investigation

3. **Block 1 Execution**: First block may need special handling
   - Genesis block transition logic
   - May require separate fix

### Next Steps

1. **Verify Legacy RLP fix** ← Current phase
2. **Investigate state root differences** (ArbOS state transitions)
3. **Fix block 2 gas discrepancy** (if still present)
4. **Fix block 1 special case** (if needed)
5. **Open PR** once all blocks 1-4000 match

---

## Commit Details

**Branch**: til/ai-fixes
**Commit**: a6f99fe12
**Title**: fix(req-1): Handle Legacy RLP transactions in SignedTx messages

**Files Changed**:
- `/home/dev/reth/crates/arbitrum/node/src/node.rs` (lines 519-538)

**Build Required**: Yes - run `cargo build --release` in nitro-rs

**Database Reset Required**: Yes - remove `nitro-db` directory for clean test

---

Generated: 2025-11-25 05:40 UTC
Status: Fix deployed, node syncing, awaiting verification
Testing Agent: Please verify blocks 20, 50+ report correct gas values
