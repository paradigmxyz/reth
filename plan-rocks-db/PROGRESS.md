# RocksDB Historical Indexes Implementation - Progress Report

**Iteration**: 1
**Date**: 2026-01-12
**Branch**: `yk/full_rocks` (same as `yk/pr3-rocksdb-history-routing`)

## Summary

This is a Ralph loop implementation tracking progress on completing RocksDB support for historical indexes in Reth (issue #20384).

## Completed Work

###  1. Issue Investigation ✅

**Findings**:
- Issue #20384 has 3 open/reopened sub-issues that need completion:
  - **#20388**: Use EitherReader/EitherWriter in DatabaseProvider and HistoricalStateProvider (REOPENED)
  - **#20390**: Modify IndexStorageHistoryStage to use EitherWriter for RocksDB writes (OPEN)
  - **#20393**: Add CLI flags to enable RocksDB storage (REOPENED)

- Current branch already has significant RocksDB work:
  - `EitherWriter::new_storages_history()` exists ✅
  - `EitherWriter::new_accounts_history()` exists ✅
  - `EitherWriter::new_transaction_hash_numbers()` exists ✅
  - Corresponding `EitherReader` methods exist ✅
  - `TransactionLookupStage` already uses EitherWriter ✅ (#20389 completed)

### 2. Fix #20393: CLI Flags ✅

**File Modified**: `crates/node/core/src/args/static_files.rs`

**Changes Made**:
1. Added three new CLI flags:
   - `--storage.tx-hash-in-rocksdb` (boolean)
   - `--storage.storages-history-in-rocksdb` (boolean)
   - `--storage.account-history-in-rocksdb` (boolean)

2. Updated `to_settings()` method to propagate these flags to `StorageSettings`:
   ```rust
   .with_transaction_hash_numbers_in_rocksdb(self.tx_hash_in_rocksdb)
   .with_storages_history_in_rocksdb(self.storages_history_in_rocksdb)
   .with_account_history_in_rocksdb(self.account_history_in_rocksdb)
   ```

**Status**: Code complete, needs testing

---

## In Progress Work

### 3. Fix #20390: IndexStorageHistoryStage and IndexAccountHistoryStage

**Problem Identified**:

The stages currently use `load_history_indices()` helper function which:
- Directly creates MDBX cursors: `provider.tx_ref().cursor_write::<H>()?` (line 195 in utils.rs)
- Uses cursor operations like `seek_exact()` for reading existing shards
- Needs to be refactored to use `EitherWriter` for writing

**Challenge**:
- `RocksDBBatch` is write-only (doesn't support read-your-writes)
- The stages need to read existing shards when `append_only = false`
- Reading must happen through `RocksTx` or MDBX transaction, not through batch

**Approach Being Considered**:

Option A: Modify `load_history_indices` signature to accept `EitherWriter`
- Requires rethinking how existing shard reading works
- May need separate reader parameter for RocksDB case

Option B: Keep cursor-based approach for MDBX, add RocksDB-specific path
- More code duplication but clearer separation

Option C: Use provider methods for reading existing shards
- Read through provider transaction (works for both MDBX and RocksDB)
- Write through EitherWriter (routes to appropriate backend)

**Next Steps**:
1. Decide on approach (leaning towards Option C)
2. Implement the refactoring in `load_history_indices`
3. Update `IndexStorageHistoryStage` to use new pattern
4. Update `IndexAccountHistoryStage` to use new pattern

---

##  Remaining Work

### 4. Verify #20388 Completion

**Task**: Check if DatabaseProvider and HistoricalStateProvider are complete

**Observations from git history**:
- Commits like "feat: wire RocksDB into history lookups via EitherReader" suggest work was done
- Need to verify by:
  1. Reading DatabaseProvider implementation
  2. Checking HistoricalStateProvider
  3. Running tests to see if there are failures
  4. Understanding why issue was reopened

**Priority**: High (should be done after #20390)

### 5. Three Completion Criteria

#### Criterion 1: Local CI ⏳
- Run `cargo +nightly fmt --all` ✅ (done, only my changes)
- Run `RUSTFLAGS="-D warnings" cargo +nightly clippy --workspace --all-features --locked` (in progress)
- Run `cargo nextest run --features "asm-keccak ethereum edge" --locked --workspace --exclude ef-tests`
- Fix any failures

#### Criterion 2: Remote CI ⏳
- Enable RocksDB flags in `metadata.rs::edge()` function (lines 47-49)
- Push to remote and monitor CI
- Fix any CI failures

#### Criterion 3: Hoodi Integration Test ⏳
- Create integration test script (`scripts/test_rocksdb_hoodi.sh`)
- Run script with existing Hoodi snapshot
- Verify no RocksDB errors
- Fix any runtime issues

---

## Technical Findings

### How TransactionLookupStage Uses EitherWriter

**Pattern** (from `tx_lookup.rs` lines 162-194):

```rust
// 1. Create RocksDB batch if feature enabled
#[cfg(all(unix, feature = "rocksdb"))]
let rocksdb_batch = provider.rocksdb_provider().batch();

// 2. Create EitherWriter with batch
let mut writer = EitherWriter::new_transaction_hash_numbers(provider, rocksdb_batch)?;

// 3. Use writer methods
writer.put_transaction_hash_number(hash, tx_num, append_only)?;

// 4. Extract and register batch for commit
#[cfg(all(unix, feature = "rocksdb"))]
if let Some(batch) = writer.into_raw_rocksdb_batch() {
    provider.set_pending_rocksdb_batch(batch);
}
```

### RocksDB Batch vs Transaction

- **`RocksDBBatch`**: Write-only, no read-your-writes support
- **`RocksTx`**: Full transaction with read-your-writes support
- **Use case**: Stages use batch for bulk writes, transaction for reading

### Key Methods Available

**EitherWriter** for history tables:
- `put_storage_history(key, value)`
- `delete_storage_history(key)`
- `put_account_history(key, value)`
- `delete_account_history(key)`

---

## Files Modified

1. ✅ `crates/node/core/src/args/static_files.rs` - Added CLI flags

## Files To Modify

2. ⏳ `crates/stages/stages/src/stages/utils.rs` - Refactor `load_history_indices()`
3. ⏳ `crates/stages/stages/src/stages/index_storage_history.rs` - Use EitherWriter
4. ⏳ `crates/stages/stages/src/stages/index_account_history.rs` - Use EitherWriter
5. ⏳ `crates/storage/db-api/src/models/metadata.rs` - Enable RocksDB flags (Criterion 2)

## Next Iteration Plan

1. Complete clippy check for CLI flag changes
2. Implement refactoring of `load_history_indices()`
3. Update both index history stages
4. Run local tests (Criterion 1)
5. If tests pass, enable RocksDB flags and push (Criterion 2)

---

## Questions / Blockers

1. **load_history_indices refactoring**: Need to decide on best approach for reading existing shards when using RocksDB
2. **Why was #20388 reopened?**: Need to investigate what issues remain
3. **Test coverage**: Should we add new tests or rely on existing ones?

---

## Command Reference

```bash
# Format
cargo +nightly fmt --all

# Lint
RUSTFLAGS="-D warnings" cargo +nightly clippy --workspace --all-features --locked

# Test
cargo nextest run --features "asm-keccak ethereum edge" --locked --workspace --exclude ef-tests

# Build with RocksDB
cargo build --release --features "asm-keccak ethereum edge"
```
