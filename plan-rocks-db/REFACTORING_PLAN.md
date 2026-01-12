# Refactoring Plan: load_history_indices for RocksDB Support

## Problem

The `load_history_indices` function currently:
1. Creates a write cursor directly (line 195)
2. Uses cursor for both reading existing shards (line 235: `seek_exact`) and writing (via `load_indices`)
3. This doesn't work with RocksDB batch (write-only)

## Solution

Separate reading and writing:
- **Reading**: Use provider's cursor/transaction (works for both MDBX and RocksDB)
- **Writing**: Use EitherWriter (routes to MDBX cursor or RocksDB batch)

## Implementation Steps

### 1. Update `load_indices` function signature

**Before**:
```rust
pub(crate) fn load_indices<H, C, P>(
    cursor: &mut C,
    partial_key: P,
    list: &mut Vec<BlockNumber>,
    sharded_key_factory: &impl Fn(P, BlockNumber) -> <H as Table>::Key,
    append_only: bool,
    mode: LoadMode,
) -> Result<(), StageError>
where
    C: DbCursorRO<H> + DbCursorRW<H>,
    H: Table<Value = BlockNumberList>,
    P: Copy,
```

**After**:
```rust
pub(crate) fn load_indices<H, P, CURSOR, N>(
    writer: &mut EitherWriter<'_, CURSOR, N>,
    partial_key: P,
    list: &mut Vec<BlockNumber>,
    sharded_key_factory: &impl Fn(P, BlockNumber) -> <H as Table>::Key,
    append_only: bool,
    mode: LoadMode,
) -> Result<(), StageError>
where
    H: Table<Value = BlockNumberList>,
    P: Copy,
    N: NodePrimitives,
    CURSOR: DbCursorRW<H> + DbCursorRO<H>,
```

### 2. Update `load_indices` writing operations

Replace cursor operations with EitherWriter methods:

**StoragesHistory**:
- `cursor.append(key, &value)?` → `writer.put_storage_history(key, &value)?`
- `cursor.upsert(key, &value)?` → `writer.put_storage_history(key, &value)?`

**AccountsHistory**:
- `cursor.append(key, &value)?` → `writer.put_account_history(key, &value)?`
- `cursor.upsert(key, &value)?` → `writer.put_account_history(key, &value)?`

### 3. Update `load_history_indices` signature

Add provider trait bounds and RocksDB support:

```rust
pub(crate) fn load_history_indices<Provider, H, P>(
    provider: &Provider,
    mut collector: Collector<H::Key, H::Value>,
    append_only: bool,
    sharded_key_factory: impl Clone + Fn(P, u64) -> <H as Table>::Key,
    decode_key: impl Fn(Vec<u8>) -> Result<<H as Table>::Key, DatabaseError>,
    get_partial: impl Fn(<H as Table>::Key) -> P,
) -> Result<(), StageError>
where
    Provider: DBProvider<Tx: DbTxMut>
        + NodePrimitivesProvider
        + StorageSettingsCache
        + RocksDBProviderFactory,
    H: Table<Value = BlockNumberList>,
    P: Copy + Default + Eq,
```

### 4. Refactor `load_history_indices` body

Create separate reader and writer:

```rust
// Create reader cursor for checking existing shards
let mut read_cursor = provider.tx_ref().cursor_read::<H>()?;

// Create RocksDB batch if feature enabled
#[cfg(all(unix, feature = "rocksdb"))]
let rocksdb_batch = provider.rocksdb_provider().batch();
#[cfg(not(all(unix, feature = "rocksdb")))]
let rocksdb_batch = ();

// Create EitherWriter based on table type
let mut writer = match () {
    _ if TypeId::of::<H>() == TypeId::of::<tables::StoragesHistory>() => {
        EitherWriter::new_storages_history(provider, rocksdb_batch)?
    }
    _ if TypeId::of::<H>() == TypeId::of::<tables::AccountsHistory>() => {
        EitherWriter::new_accounts_history(provider, rocksdb_batch)?
    }
    _ => unreachable!("load_history_indices only works with history tables"),
};

// ... rest of function using read_cursor for reading and writer for writing
```

## Challenges

1. **TypeId matching**: Need a way to determine which EitherWriter constructor to call based on generic `H`
   - Could use separate functions for each table type
   - Or pass writer factory function as parameter

2. **Cursor trait bounds**: EitherWriter needs the cursor type in its signature
   - This makes the signature more complex
   - May need intermediate type aliases

## Alternative Approach: Table-Specific Functions

Instead of making `load_history_indices` fully generic, create specialized versions:

```rust
pub(crate) fn load_storages_history_indices<Provider, P>(...) -> Result<(), StageError>
pub(crate) fn load_accounts_history_indices<Provider, P>(...) -> Result<(), StageError>
```

This is cleaner and avoids TypeId matching, but requires some code duplication.

## Decision: Use Table-Specific Functions

**Rationale**:
- Clearer code with no TypeId matching
- Easier to maintain
- Slight code duplication is acceptable for clarity
- Follows Rust principle of explicitness over cleverness

## Implementation Order

1. ✅ Add CLI flags (completed)
2. ⏳ Create `load_storages_history_indices` function
3. ⏳ Create `load_accounts_history_indices` function
4. ⏳ Update `IndexStorageHistoryStage` to use new function
5. ⏳ Update `IndexAccountHistoryStage` to use new function
6. ⏳ Test and fix any issues
7. ⏳ Run Criterion 1 tests
