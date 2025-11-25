# CRITICAL: Transaction Type Misidentification Bug

## Summary

Block 20 transactions are being misidentified, causing incorrect gas calculations and receipts.

## The Issue

**Block 20 Official Chain:**
- TX 0: Type 0x6a (Internal) ✅
- TX 1: Type 0x2 (EIP-1559) with 59,039 gas (0xe79f) ✅

**Block 20 Our Node:**
- TX 0: Type Internal ✅
- TX 1: Type Deposit ❌ WRONG!

## Evidence

From node logs:
```
[2025-11-25T05:02:00] follower: executing tx tx_type=Deposit tx_hash=0x224081df...
[2025-11-25T05:02:00] finalized_tx_types=["Internal", "Deposit"]
```

From official chain:
```bash
curl official_rpc -d '{"method":"eth_getBlockByNumber","params":["0x14",true],...}'
TX 1 type: 0x2  # EIP-1559, NOT Deposit
TX 1 gas: 0xe79f  # 59,039 gas
```

## Impact

1. **Gas Calculation**: Deposit transactions don't add to cumulative gas (or do they?)
2. **Receipt Type**: Wrong receipt variant created
3. **RPC Serialization**: Transaction shows no type field in eth_getBlockByNumber
4. **Block Hash**: Wrong transactions → wrong transaction root → wrong block hash

## Root Cause

The transaction type is determined when parsing messages from L1. The issue is likely in:

1. **Message Parsing**: `/home/dev/arb-alloy` or `/home/dev/nitro-rs` message decoder
2. **Transaction Construction**: How we convert decoded messages to `ArbTransactionSigned`

## Investigation Needed

### 1. Check Message Decoding

Look at how message #4607 (which creates block 20) is being parsed. The message should contain TWO transactions:
- One Internal (0x6a)
- One EIP-1559 (0x2)

But we're parsing the second as Deposit (0x64).

### 2. Check Official Go Implementation

In `/home/dev/nitro`, find how transactions are decoded from messages:
- `arbos/` package - transaction handling
- Message format and transaction type bytes
- How transaction type is determined from message bytes

### 3. Compare Transaction Hashes

Both chains have the same transaction hash (0x224081df...), which means:
- The transaction data is the SAME
- But we're interpreting the type DIFFERENTLY

This suggests the bug is in how we read the type byte from the transaction encoding.

## Hypothesis

EIP-1559 transactions (type 0x02) might be getting confused with Deposit transactions (type 0x64).

Wait, that doesn't make sense because 0x02 ≠ 0x64. Let me check if there's a mapping issue or if the message format has a different encoding for transactions.

## Next Steps

1. **Devin MCP Query**: Ask about transaction type encoding in Arbitrum messages
2. **Compare Encodings**: Get the raw transaction bytes for TX 1 from both chains
3. **Trace Decoding**: Add logging to show exactly how the transaction type is determined
4. **Fix Parser**: Update the transaction type detection logic

## Related Issues

This bug also explains why:
- Blocks 50+ report 0 gas (likely all contain misidentified Deposit transactions)
- Receipt types don't match
- RPC responses have missing fields (Deposit transactions have fewer fields than EIP-1559)

---

Generated: 2025-11-25 05:05 UTC
Node: PID 250756
Status: CRITICAL - Transaction type parsing bug found
Priority: HIGH - Blocks all progress on gas/receipt/hash matching
