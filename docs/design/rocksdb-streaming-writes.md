# Design: RocksDB Streaming Writes for Pipeline Stages

## Problem

TransactionHashNumbers and History stages write huge batches to RocksDB:
- TransactionHashNumbers: 65M+ entries in one batch
- This causes OOM on memory-constrained systems
- Current flow: ETL sorts → giant WriteBatch → single commit

## Constraints

1. **Sorted writes are important** - RocksDB performs better with sorted keys
2. **Crash safety** - If crash happens mid-write, we need a recovery strategy
3. **No complex healing logic** - User explicitly doesn't want unwind/healing code

## Solution: Idempotent Stage + Streaming Writes

### Key Insight: "Rebuild, Don't Resume"

Make stages **pure and idempotent**:
1. On start, `DeleteRange` the entire key range for this stage
2. Stream sorted ETL output in bounded WriteBatches
3. On crash, simply re-run the stage (it clears and rebuilds)

This avoids complex checkpoint/healing logic by always rebuilding from source (MDBX).

## Implementation

### 1. Add `clear()` method to RocksDBProvider

```rust
impl RocksDBProvider {
    /// Clears all entries from the specified table.
    /// Used at stage start to ensure idempotency.
    pub fn clear<T: Table>(&self) -> ProviderResult<()>;
}
```

### 2. Add streaming batch writer

```rust
/// Writes entries to RocksDB in bounded batches to avoid OOM.
pub struct RocksDBBatchWriter<'a, T: Table> {
    provider: &'a RocksDBProvider,
    batch: WriteBatch,
    batch_size: usize,
    max_batch_size: usize,  // e.g., 100k entries or 64MB
}

impl<T: Table> RocksDBBatchWriter<'_, T> {
    /// Adds an entry to the batch, flushing if limit reached.
    pub fn put(&mut self, key: T::Key, value: T::Value) -> ProviderResult<()> {
        self.batch.put(...);
        self.batch_size += 1;
        if self.batch_size >= self.max_batch_size {
            self.flush()?;
        }
        Ok(())
    }
    
    /// Flushes remaining entries.
    pub fn finish(self) -> ProviderResult<()>;
}
```

### 3. Update TransactionLookup stage

```rust
fn execute(&mut self, ...) -> Result<...> {
    // Clear existing data for idempotency
    provider.rocksdb_provider().clear::<TransactionHashNumbers>()?;
    
    // Stream ETL output in bounded batches
    let mut writer = provider.rocksdb_provider()
        .batch_writer::<TransactionHashNumbers>()
        .with_max_entries(100_000);
    
    for (hash, tx_num) in etl.drain_sorted() {
        writer.put(hash, tx_num)?;
    }
    
    writer.finish()?;
    Ok(...)
}
```

### 4. Update History stages similarly

Same pattern for AccountsHistory and StoragesHistory.

## Crash Safety

| Scenario | Behavior |
|----------|----------|
| Crash before stage starts | No change, normal retry |
| Crash during clear | Partial clear, but next run will clear again |
| Crash during writes | Partial data, but next run clears and rebuilds |
| Crash after finish | Success, data persisted |

**Key point**: ETL data is re-derivable from MDBX. We don't need to preserve ETL temp files across crashes.

## Memory Usage

| Current | With Streaming |
|---------|----------------|
| O(total_entries) | O(batch_size + ETL_buffer) |
| 65M entries → OOM | 100k batch → ~50MB peak |

## Trade-offs

| Aspect | Trade-off |
|--------|-----------|
| Crash recovery | Rebuilds entire stage range (slower but simpler) |
| Write amplification | Multiple small batches vs one large (similar) |
| Code complexity | Much simpler than checkpoint/healing logic |

## Configuration

```rust
/// Default batch size for RocksDB streaming writes.
const DEFAULT_ROCKSDB_BATCH_SIZE: usize = 100_000;

/// Maximum batch size in bytes.
const DEFAULT_ROCKSDB_BATCH_BYTES: usize = 64 * 1024 * 1024; // 64MB
```

## Future Improvements

If full-stage rebuild is too slow:
1. Add coarse checkpoints (every N blocks)
2. Use SST file ingestion for even faster bulk loads

## Files to Modify

1. `crates/storage/provider/src/providers/rocksdb/provider.rs`
   - Add `clear<T>()` method
   - Add `RocksDBBatchWriter` struct

2. `crates/stages/stages/src/stages/tx_lookup.rs`
   - Clear at start, use streaming writer

3. `crates/stages/stages/src/stages/index_account_history.rs`
   - Same pattern

4. `crates/stages/stages/src/stages/index_storage_history.rs`
   - Same pattern
