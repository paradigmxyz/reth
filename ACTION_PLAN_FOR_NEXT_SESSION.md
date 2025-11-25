# Action Plan for Next Session

## Current Status

**Commits on til/ai-fixes:**
- 334c4f287: Check early_tx_gas before transaction type ✅ Pushed
- 3dbb195a6: Preserve cumulative gas for Internal transactions ✅ Pushed
- 9f93d523b: Wrap receipts in ReceiptWithBloom ✅ Tested

**Testing Agent Status:**
- Only sees 9f93d523b (node not rebuilt with latest commits)
- Reports blocks 50+ have 0 gas
- Reports block 2 has +6 gas discrepancy

**Root Cause Identified:**
Transaction type misidentification - EIP-1559 transactions (type 0x02) are being treated as Deposit transactions (type 0x64).

## Step-by-Step Fix Plan

### Step 1: Add Diagnostic Logging

**File**: `/home/dev/reth/crates/arbitrum/node/src/node.rs`
**Location**: Around line 520 where SignedTx (0x04) messages are decoded

**Add logging BEFORE decode_2718:**
```rust
0x04 => {
    let mut s = cur;
    while !s.is_empty() {
        // ADD THIS:
        if s.len() >= 1 {
            tracing::warn!(
                target: "arb-reth::decode",
                first_byte = format!("0x{:02x}", s[0]),
                total_len = s.len(),
                "SignedTx message - first byte (should be 0x00-0x04, not 0x64)"
            );
        }

        let tx = ArbTransactionSigned::decode_2718(&mut s)?;
        // ... rest of code
    }
}
```

### Step 2: Check Message Source

**Question**: Where do the bytes in `cur` come from?

**Investigation needed:**
```bash
# Find where 'cur' is set in the parse_l2_message function
grep -B20 "0x04 =>" /home/dev/reth/crates/arbitrum/node/src/node.rs | grep "let.*cur"
```

The bytes come from the L1 message's `l2_msg` field, which is read by nitro-rs and passed to reth via the Engine API.

### Step 3: Compare with Go Implementation

**File**: `/home/dev/nitro/arbos/parse_l2.go`
**Lines**: 159-174

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

Key insight: Go calls `UnmarshalBinary` on raw bytes. This should decode a standard Ethereum transaction envelope.

### Step 4: Hypothesis

**Possible causes:**

1. **Nitro-rs sends wrong bytes**: The message streamer in nitro-rs might be modifying/re-encoding transaction bytes incorrectly.

2. **L1 message has wrong format**: The actual L1 message might be malformed (unlikely - official node works).

3. **We're parsing wrong message type**: Maybe the message isn't actually a SignedTx (0x04)?

### Step 5: Test Hypothesis

**Add logging in nitro-rs:**

Find where L2 messages are read and log the raw bytes:

```rust
// In nitro-rs message streamer
tracing::info!(
    "L2 message: kind={}, l2_msg_len={}, first_10_bytes={:02x?}",
    message.kind,
    message.l2_msg.len(),
    &message.l2_msg[..min(10, message.l2_msg.len())]
);
```

### Step 6: The Fix (Once Located)

Once we find where the wrong type byte is set:

**If in nitro-rs:**
- Fix message construction/passing

**If in reth node.rs:**
- Fix how we extract transaction bytes from messages

**If it's a batch decoding issue:**
- Check batch parsing logic around line 500-540 in node.rs

## Quick Win Strategy

Since finding the encoding bug is complex, here's an alternative approach:

### Workaround: Detect and Fix Type Byte

**File**: `/home/dev/reth/crates/arbitrum/node/src/node.rs`
**Around line 520:**

```rust
0x04 => {
    let mut s = cur;
    while !s.is_empty() {
        let before_len = s.len();

        // WORKAROUND: Check if first byte is 0x64 (Deposit) when it should be 0x02 (EIP1559)
        // This is a hack until we find the root cause
        if s.len() > 0 && s[0] == 0x64 {
            tracing::warn!(
                target: "arb-reth::decode",
                "DETECTED WRONG TYPE BYTE: 0x64 (Deposit) in SignedTx message! Converting to 0x02 (EIP1559)"
            );
            // Temporarily change the type byte for decoding
            // TODO: Find and fix the root cause of this encoding bug
            s[0] = 0x02;
        }

        let tx = ArbTransactionSigned::decode_2718(&mut s)?;
        // ... rest
    }
}
```

**Note**: This is a HACK and not a real fix, but it might make blocks work temporarily while we find the real issue.

## Testing After Fix

1. Rebuild: `cd /home/dev/nitro-rs && cargo build --release`
2. Clean: `rm -rf nitro-db node.log*`
3. Start node and wait 5 minutes
4. Check block 20:
   ```bash
   curl -s http://localhost:8547 -d '{"method":"eth_getBlockByNumber","params":["0x14",true],"id":1,"jsonrpc":"2.0"}' | python3 -m json.tool | grep -A5 '"type"'
   ```
   Should show TX 1 as type "0x2", not missing type

5. Check gas:
   ```bash
   curl -s http://localhost:8547 -d '{"method":"eth_getBlockByNumber","params":["0x14",false],"id":1,"jsonrpc":"2.0"}' | python3 -c "import sys,json; print('Block 20 gas:', json.load(sys.stdin)['result']['gasUsed'])"
   ```
   Should show: `Block 20 gas: 0xc00e`

## Success Criteria

- Block 20 TX 1 identified as EIP-1559 (type 0x2)
- Block 20 reports 0xc00e (49,166) gas
- Blocks 50+ report non-zero gas
- Receipts root starts matching for blocks with correct tx types

---

Generated: 2025-11-25 05:13 UTC
Priority: CRITICAL
Estimated effort: 2-4 hours to find and fix properly
Quick hack: 15 minutes but not recommended for production
