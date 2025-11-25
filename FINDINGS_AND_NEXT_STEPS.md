# Findings and Next Steps - Iteration 71

## What I've Confirmed

###  Go Nitro Architecture ✅

From `/home/dev/nitro/arbos/block_processor.go`:

```go
// 1. Parse user transactions from L1 message
txes, err := ParseL2Transactions(message, chainConfig.ChainID, lastArbosVersion)

// 2. Create StartBlock Internal transaction
startTx := InternalTxStartBlock(chainConfig.ChainID, l1Header.L1BaseFee, l1BlockNum, header, lastBlockHeader)
firstTx := types.NewTx(startTx)

// 3. Execute in order:
if firstTx != nil {
    tx = firstTx
    firstTx = nil
} else if len(redeems) > 0 {
    tx = redeems[0]
    redeems = redeems[1:]
} else {
    tx, err = sequencingHooks.NextTxToSequence()  // User transactions
}
```

**Key Point**: StartBlock tx is PREPENDED before all other transactions.

### Our Rust Implementation ✅

#### Transaction Parsing (`node.rs`)
- ✅ `parse_l2_message_to_txs()` parses user transactions from L1 messages
- ✅ Returns empty Vec for EndOfBlock messages (kind=6)
- ✅ Similar to Go's `ParseL2Transactions`

#### ArbOS Hooks (`execute.rs`)
- ✅ `DefaultArbOsHooks::start_tx()` handles Internal transactions (0x6A)
- ✅ Unpacks StartBlock data when selector matches `startBlock(uint256,uint64,uint64,uint64)`
- ✅ Processes block metadata (L1 block number, base fee, etc.)
- ✅ Similar to Go's ArbOS hooks

## What Still Needs Investigation

### Critical Question: Transaction Ordering

**In Go**: StartBlock is explicitly prepended as `firstTx` before user transactions

**In Rust**: Need to verify:
1. Where is the StartBlock Internal transaction created as an actual transaction?
2. Is it added to the transaction list?
3. Is it prepended before user transactions?
4. Are the transaction bytes/hash correct?

### Hypothesis for Transaction Hash Mismatch

From iteration 69, we found that blocks 13+ have different transaction hashes than official chain. Possible causes:

1. **StartBlock not created**: Block missing the Internal tx entirely
2. **StartBlock wrong data**: Transaction created with incorrect fields
3. **StartBlock wrong order**: Created but not prepended correctly
4. **StartBlock wrong encoding**: Transaction bytes don't match Go implementation

### Where to Look Next

####  Block Production Flow

Need to trace from `/home/dev/reth/crates/arbitrum/node/src/node.rs`:
1. L1 message arrives
2. `parse_l2_message_to_txs()` extracts user transactions
3. **??? Where does StartBlock Internal tx get created?**
4. **??? Where are StartBlock + user txs combined?**
5. Block executor receives transaction list
6. `start_tx()` hook processes each transaction

#### Key Files to Check

1. `/home/dev/reth/crates/arbitrum/node/src/node.rs` - Block production
2. `/home/dev/reth/crates/arbitrum/evm/src/execute.rs` - ArbOS hooks
3. `/home/dev/reth/crates/arbitrum/evm/src/internal_tx.rs` - StartBlock transaction creation
4. `/home/dev/reth/crates/arbitrum/evm/src/config.rs` - Block assembly

####  Specific Investigation Steps

1. **Search for where Internal transactions are created**:
   ```bash
   grep -r "ArbTxType::Internal\|0x6A\|InternalTx" crates/arbitrum/
   ```

2. **Find where transaction lists are built**:
   ```bash
   grep -r "Vec.*Transaction\|transactions.push\|transactions.insert" crates/arbitrum/node/
   ```

3. **Check if StartBlock tx is prepended**:
   ```bash
   grep -r "prepend\|firstTx\|first.*transaction" crates/arbitrum/
   ```

4. **Compare StartBlock transaction data**:
   - Run node with logging
   - Check StartBlock tx hash in logs
   - Compare with official chain block 1 tx 0

## Test Plan

Once we confirm/fix how StartBlock transactions are created:

1. **Build**: `cargo build --release` in nitro-rs
2. **Clean**: Remove `nitro-db/` and old logs
3. **Run**: Start node with logging
4. **Wait**: 5 minutes for first blocks
5. **Check Block 1**:
   ```bash
   curl localhost:8547 -X POST -H "Content-Type: application/json" \
     --data '{"method":"eth_getBlockByNumber","params":["0x1",true],"id":1,"jsonrpc":"2.0"}'
   ```
6. **Compare**:
   - Transaction count (should be 3)
   - First tx hash (should be StartBlock Internal)
   - gasUsed (should be 0x1d8a8 = 121000)
   - Block hash (should match official)

## Status Summary

**Commits on til/ai-fixes**: 8 total
- Recent: 22aee7248 (Transaction sourcing architecture analysis)

**What Works**:
- ✅ Legacy RLP transaction decoding (commit a6f99fe12)
- ✅ Receipt gas correction in build.rs
- ✅ ArbOS hooks handle StartBlock transactions
- ✅ Internal tx data unpacking

**What's Unknown**:
- ❓ Is StartBlock tx created as actual transaction object?
- ❓ Is it prepended to user transactions?
- ❓ Is the transaction data/hash correct?

**What's Broken (from iteration 69)**:
- ❌ Blocks 13+ have wrong transaction hashes
- ❌ This causes wrong gas, state roots, block hashes
- ❌ req-1 fails: blocks don't match official chain

## Recommendation for Testing Agent

The investigation has narrowed down to a specific architectural question: **How are StartBlock Internal transactions created and ordered in our Rust implementation?**

This requires:
1. Deeper code tracing (time-consuming)
2. Potentially adding more diagnostic logging
3. Running the node and comparing transaction lists

**Suggested approach**:
1. Add logging where transactions are combined/ordered
2. Rebuild and run node
3. Check if block 1 has 3 transactions with correct hashes
4. If not, identify where StartBlock tx is missing/wrong

**Alternative approach**:
- Focus on blocks 1-12 which DO work correctly
- Understand why they succeed
- Apply same pattern to blocks 13+

---

Generated: 2025-01-13 (Iteration 71)
Status: Investigation in progress
Next: Trace transaction creation and ordering
Priority: CRITICAL for req-1
