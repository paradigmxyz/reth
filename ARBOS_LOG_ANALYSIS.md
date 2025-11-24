# ArbOS Log Emission Analysis

## Issue Summary

Block comparison tests show that submit retryable transactions (type 0x69) are missing required event logs in receipts:
- Local: NO LOGS emitted
- Reference: 2 logs emitted (TicketCreated and RedeemScheduled)

## Code Analysis

### Log Emission (execute.rs)

The logs ARE being emitted correctly in `crates/arbitrum/evm/src/execute.rs`:

```rust
// Line 939: TicketCreated event
crate::log_sink::push(ARB_RETRYABLE_TX_ADDRESS, &[TICKET_CREATED_TOPIC, ticket_id.0], &[]);

// Line 991: RedeemScheduled event
crate::log_sink::push(ARB_RETRYABLE_TX_ADDRESS, &[REDEEM_SCHEDULED_TOPIC, ticket_id.0, retry_tx_hash.0, sequence_num_bytes], &redeem_data);
```

These are emitted during `start_tx` handling for transaction type 0x69 (submit retryable).

### Log Sink Infrastructure (log_sink.rs)

The log sink uses thread-local storage and has three operations:
- `push()`: Adds a log to thread-local storage (line 12-26)
- `take()`: Retrieves and clears all logs (line 28-40)
- `clear()`: Clears all logs (line 8-10)

### Log Retrieval (receipts.rs)

Logs are correctly retrieved in the receipt builder:

```rust
// Line 85-90
let mut logs = ctx.result.into_logs();  // EVM logs
let mut extra = crate::log_sink::take();  // Predeploy logs
if !extra.is_empty() {
    logs.append(&mut extra);
}
```

### Transaction Flow (build.rs)

For submit retryable transactions:
1. `start_tx()` is called, which emits logs via `log_sink::push()` (execute.rs:939, 991)
2. `start_hook_result.end_tx_now` is set to `true` (execute.rs:993-997)
3. Predeploy dispatch is SKIPPED because `end_tx_now` is true (build.rs:343)
4. Log sink is NOT cleared (clear() is only called at line 360, inside the `if !end_tx_now` block)
5. EVM is executed anyway (build.rs:456) to generate a receipt
6. Receipt builder retrieves logs via `log_sink::take()` (receipts.rs:87)

**This should work!** The logs are emitted, not cleared, and retrieved.

## Potential Issues

### 1. Thread-Local Storage
The log_sink uses thread_local storage. If different parts of the execution happen on different threads, logs could be lost. However, the entire `execute_transaction` flow should be on the same thread.

### 2. Compilation Errors
The codebase has compilation errors in `reth-arbitrum-node`:
```
error[E0271]: type mismatch resolving `<ArbReceiptConverter<...> as ReceiptConverter<...>>::RpcReceipt == TransactionReceipt`
error[E0277]: the trait bound `ArbEthApi<NodeAdapter<N, ...>, ...>: RpcNodeCore` is not satisfied
```

These errors prevent building and testing the changes.

### 3. Log Sink State
Need to verify that `log_sink::take()` is only called once per transaction. If it's called multiple times, the second call would return empty logs.

## Other Receipt Issues

### Gas Used Discrepancies
- Transaction 1 (internal 0x6a): Local 0x54d8 vs Reference 0x0
- Transaction 2 (submit retryable 0x69): Local 0x131c8 vs Reference 0x186a0
- Transaction 3 (retry 0x68): Both 0x5208 ✓

The internal transaction should have gasUsed=0 according to line 54 in receipts.rs, but something is overriding this.

### From Address
Recent commits have attempted to fix this:
- commit b45546680: "Fix transaction receipt 'from' field by recovering signer directly"
- commit 6e0acfe11: "recover actual signer for receipt 'from' field"

Need to verify these fixes work correctly.

## Next Steps

1. **Fix Compilation Errors**: Resolve type mismatches in reth-arbitrum-node
2. **Add Logging**: Add tracing to verify log_sink operations:
   - When logs are pushed
   - When logs are taken
   - How many logs are in the sink
3. **Test with Working Build**: Rebuild and test to see if logs appear
4. **Investigate Gas Usage**: Check why internal transactions aren't reporting 0 gas
5. **Verify From Field**: Test that recent fixes for 'from' address work correctly

## References

- Issue first identified in test report iteration 19
- Block 1 transaction 2 (0x13cb79b0...) should have 2 logs but has 0
- Reference Arbitrum Sepolia: https://arb-sepolia.g.alchemy.com/v2/SZtN3OkrMwNmsXx5x-4Db
