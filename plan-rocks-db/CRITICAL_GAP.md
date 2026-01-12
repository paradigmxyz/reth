# CRITICAL GAP IDENTIFIED: Unwind Operations

**Date**: 2026-01-12 23:32 UTC
**Priority**: HIGH

## Problem

The index history stages' unwind operations are NOT using EitherWriter for RocksDB:

### Current Unwind Implementation

**IndexStorageHistoryStage** (line 151):
```rust
provider.unwind_storage_history_indices_range(BlockNumberAddress::range(range))?;
```

**IndexAccountHistoryStage** (similar):
```rust
provider.unwind_account_history_indices_range(range)?;
```

These call provider methods that use **direct MDBX cursors** (lines 2868, 2923 in provider.rs):
```rust
let mut cursor = self.tx.cursor_write::<tables::AccountsHistory>()?;
let mut cursor = self.tx.cursor_write::<tables::StoragesHistory>()?;
```

**Result**: Unwind will NOT work with RocksDB!

## Correct Pattern (from TransactionLookupStage)

TransactionLookupStage handles unwind correctly (lines 220-255):
```rust
// Create RocksDB batch
let rocksdb = provider.rocksdb_provider();
let rocksdb_batch = rocksdb.batch();

// Create EitherWriter
let mut writer = EitherWriter::new_transaction_hash_numbers(provider, rocksdb_batch)?;

// Delete entries
writer.delete_transaction_hash_number(hash)?;

// Register batch
if let Some(batch) = writer.into_raw_rocksdb_batch() {
    provider.set_pending_rocksdb_batch(batch);
}
```

## Root Cause Analysis

### Why Issue #20391 Was Closed

Issue #20391 ("Implement unwind for RocksDB") was closed with comment:
> "already covered in #20389 and #20390"

**Interpretation**:
- #20389: TransactionLookupStage (implements unwind with EitherWriter ✅)
- #20390: Index history stages (MY WORK - but I didn't implement unwind! ❌)

### Relationship to Issue #20388

Issue #20388 is about "Use EitherReader/EitherWriter in DatabaseProvider and HistoricalStateProvider"

This suggests the provider methods should be updated to use EitherWriter, which would fix unwind!

**Two possible solutions**:
1. Update provider methods to use EitherWriter (#20388's scope)
2. Make stages handle unwind directly (like TransactionLookupStage)

## Current Implementation Status

### What I Implemented ✅
- Execute path uses EitherWriter ✅
- Load functions use EitherWriter ✅
- All forward operations work with RocksDB ✅

### What's Missing ❌
- Unwind operations don't use EitherWriter ❌
- Will only work with MDBX, not RocksDB ❌
- Critical for full RocksDB support ❌

## Impact Assessment

### Severity: HIGH

**Forward Operations** (execute):
- ✅ Will work with RocksDB
- ✅ Indices will be written correctly

**Backward Operations** (unwind):
- ❌ Will fail or corrupt data if RocksDB is enabled
- ❌ Will try to delete from MDBX when data is in RocksDB

### Test Coverage

The existing tests likely don't catch this because:
- Tests may not enable RocksDB feature
- Unwind tests might be using MDBX path
- Integration with RocksDB not fully tested

## Solutions

### Option 1: Implement in Stages (Recommended for #20390)
Follow TransactionLookupStage pattern in both index history stages:

**Pros**:
- Stages own their logic
- Consistent with TransactionLookupStage
- Doesn't modify provider layer

**Cons**:
- Need to replicate `unwind_history_shards` logic with EitherWriter
- More complex implementation
- Some code duplication

### Option 2: Fix Provider Methods (Part of #20388)
Update `HistoryWriter` trait implementation to use EitherWriter:

**Pros**:
- Centralized logic
- Stages don't change
- Fixes #20388 at same time

**Cons**:
- Modifies provider layer (broader impact)
- Might be out of scope for #20390
- Requires more testing

## Recommended Action

### For Ralph Loop Completion

**Immediate** (Iteration 4):
1. Implement Option 1: Add EitherWriter unwind to stages
2. Create helper function for unwind with EitherWriter
3. Update both stages to use new unwind implementation
4. Test unwind operations thoroughly

**Future** (Issue #20388):
1. Consider refactoring provider methods
2. Centralize unwind logic if appropriate
3. Coordinate with #20388 work

## Implementation Plan

### New Function Needed
```rust
fn unwind_history_shards_via_writer<P, CURSOR, N>(
    reader: &mut CURSOR,  // For reading shards
    writer: &mut EitherWriter<'_, CURSOR, N>,  // For writing
    key: ShardedKey,
    block_number: BlockNumber,
) -> ProviderResult<Vec<u64>>
where...
```

### Stage Updates
Both `IndexStorageHistoryStage::unwind()` and `IndexAccountHistoryStage::unwind()` need to:
1. Create RocksDB batch
2. Create EitherWriter
3. Create read cursor
4. Call new unwind helper
5. Register batch

## Status

- ❌ Unwind NOT implemented with EitherWriter
- ⏳ Blocks full RocksDB support
- 🔴 MUST FIX before Criterion 2/3 can truly pass

---

**Priority**: Implement in next iteration before integration testing
**Blocker**: Yes - integration test will likely fail on unwind operations
