# RocksDB Initialization and Consistency Check Plan

**Issue:** [#20392 - Initialize RocksDB provider and add consistency check on startup](https://github.com/paradigmxyz/reth/issues/20392)  
**Related PR:** [#20340 - feat(storage): add method to check invariants on RocksDB tables](https://github.com/paradigmxyz/reth/pull/20340)

## Overview

This document outlines the implementation plan to address issue #20392. The goal is to:
1. Run `RocksDBProvider::check_consistency()` on startup (mirroring static file checks)
2. Wire RocksDB provider correctly into stages that need it
3. Add configuration validation to prevent RocksDB flag changes after initial sync

## Current State Analysis

### What's Already Implemented (from PR #20340)

✅ **RocksDB Provider** (`crates/storage/provider/src/providers/rocksdb/provider.rs`):
- `RocksDBBuilder` with `with_default_tables()` to register all required column families
- Full CRUD operations with metrics support
- Transaction support (`RocksTx`) with read-your-writes semantics
- Batch operations (`RocksDBBatch`) for atomic writes

✅ **RocksDB Invariants** (`crates/storage/provider/src/providers/rocksdb/invariants.rs`):
- `check_consistency()` method that checks:
  - `TransactionHashNumbers` table against `TransactionLookup` stage checkpoint
  - `StoragesHistory` table against `IndexStorageHistory` stage checkpoint
- Auto-healing (pruning) when RocksDB is ahead of MDBX
- Returns `Option<BlockNumber>` for unwind target when RocksDB is behind

✅ **RocksDB Initialization in Launch** (`crates/node/builder/src/launch/common.rs:490-494`):
```rust
let rocksdb_provider = RocksDBProvider::builder(self.data_dir().rocksdb())
    .with_default_tables()
    .with_metrics()
    .with_statistics()
    .build()?;
```

✅ **RocksDB Wired to ProviderFactory** (`crates/node/builder/src/launch/common.rs:496-502`):
```rust
let factory = ProviderFactory::new(
    self.right().clone(),
    self.chain_spec(),
    static_file_provider,
    rocksdb_provider,  // <-- Already passed!
)?
```

### What's Missing (Issue #20392 Requirements)

❌ **Consistency Check on Startup**: `check_consistency()` is not called during node launch

❌ **Stages Using RocksDB**: Stages (`TransactionLookupStage`, `IndexStorageHistoryStage`, `IndexAccountHistoryStage`) don't have explicit RocksDB wiring - they rely on `EitherWriter` abstractions

❌ **Configuration Validation**: No check to prevent RocksDB flag changes after initial sync

---

## Implementation Plan

### Step 1: Add RocksDB Consistency Check on Startup (Priority: High)

**Location:** `crates/node/builder/src/launch/common.rs`

**Task:** After the static file consistency check (line 506-553), add RocksDB consistency check.

```rust
// After static file check...
if let Some(unwind_target) =
    factory.static_file_provider().check_consistency(&factory.provider()?)?
{
    // ... existing unwind handling
}

// NEW: RocksDB consistency check
if let Some(unwind_target) = factory.rocksdb().check_consistency(&factory.provider()?)? {
    info!(target: "reth::cli", unwind_target, "RocksDB inconsistency detected, executing unwind.");
    
    // Use same unwind pipeline pattern as static files
    let (_tip_tx, tip_rx) = watch::channel(B256::ZERO);
    let pipeline = PipelineBuilder::default()
        .add_stages(DefaultStages::new(/* ... */))
        .build(factory.clone(), StaticFileProducer::new(factory.clone(), self.prune_modes()));
    
    let (tx, rx) = oneshot::channel();
    self.task_executor().spawn_critical_blocking(
        "rocksdb unwind task",
        Box::pin(async move {
            let (_, result) = pipeline.run_as_fut(Some(PipelineTarget::Unwind(unwind_target))).await;
            let _ = tx.send(result);
        }),
    );
    rx.await?.inspect_err(|err| {
        error!(target: "reth::cli", unwind_target, %err, "failed to run RocksDB unwind")
    })?;
}
```

**Considerations:**
- Run RocksDB check **after** static file check (static files must be consistent first since RocksDB pruning uses them)
- If both checks return unwind targets, take the minimum (most conservative)
- The `check_consistency()` signature requires `Provider: DBProvider + StageCheckpointReader + StorageSettingsCache + StaticFileProviderFactory + TransactionsProvider`

**Files to modify:**
- `crates/node/builder/src/launch/common.rs`

---

### Step 2: Ensure Stages Write to RocksDB via EitherWriter (Priority: Medium)

**Current Status:** Stages already use `EitherWriter` pattern through `HistoryWriter` trait. The routing to RocksDB vs MDBX is controlled by `StorageSettings`.

**Location:** `crates/storage/provider/src/writer/mod.rs` (EitherWriter)

**Verification Tasks:**
1. Confirm `TransactionLookupStage` uses `EitherWriter` for `TransactionHashNumbers`
2. Confirm `IndexStorageHistoryStage` uses `EitherWriter` for `StoragesHistory`  
3. Confirm `IndexAccountHistoryStage` uses `EitherWriter` for `AccountsHistory`

**Current Stage Implementation Analysis:**

| Stage | Table | Current Writer | RocksDB Support |
|-------|-------|----------------|-----------------|
| `TransactionLookupStage` | `TransactionHashNumbers` | Direct cursor writes | ❓ Needs verification |
| `IndexStorageHistoryStage` | `StoragesHistory` | `HistoryWriter` trait | ❓ Needs verification |
| `IndexAccountHistoryStage` | `AccountsHistory` | `HistoryWriter` trait | ❓ Needs verification |

**Action Items:**
- Review `EitherWriter` implementation to confirm it routes to RocksDB based on `StorageSettings`
- If stages bypass `EitherWriter`, modify them to use it
- Related issues: #20390 (IndexStorageHistoryStage), #20388 (EitherReader/EitherWriter)

---

### Step 3: Configuration Validation (Priority: Medium)

**Goal:** Prevent users from changing RocksDB flags after initial sync.

**Approach:**
1. Store initial RocksDB settings in MDBX metadata on first run
2. On subsequent runs, compare current config vs stored settings
3. Error if mismatch and chain has progressed past genesis

**Location:** `crates/storage/provider/src/providers/database/mod.rs` or similar

**Implementation:**

```rust
/// Stored in MDBX metadata table
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedRocksdbSettings {
    pub transaction_hash_numbers_in_rocksdb: bool,
    pub storages_history_in_rocksdb: bool,
    pub accounts_history_in_rocksdb: bool,
    // Version for future migrations
    pub version: u8,
}

impl PersistedRocksdbSettings {
    pub fn validate_against(&self, current: &StorageSettings) -> Result<(), ConfigMismatchError> {
        if self.transaction_hash_numbers_in_rocksdb != current.transaction_hash_numbers_in_rocksdb
            || self.storages_history_in_rocksdb != current.storages_history_in_rocksdb
            || self.accounts_history_in_rocksdb != current.accounts_history_in_rocksdb
        {
            return Err(ConfigMismatchError {
                stored: self.clone(),
                current: current.clone(),
            });
        }
        Ok(())
    }
}
```

**Startup Flow:**
```rust
// In launch/common.rs or provider factory initialization
let stored_settings = provider.get_rocksdb_settings()?;
match stored_settings {
    Some(stored) => {
        // Validate if chain has progressed
        let tip = provider.last_block_number()?;
        if tip > 0 {
            stored.validate_against(&current_settings)?;
        }
    }
    None => {
        // First run - persist current settings
        provider.save_rocksdb_settings(&current_settings.into())?;
    }
}
```

**Files to modify:**
- `crates/db-api/src/models.rs` - Add `PersistedRocksdbSettings` model
- `crates/storage/provider/src/traits/...` - Add trait for reading/writing settings
- `crates/node/builder/src/launch/common.rs` - Add validation at startup

---

### Step 4: Add AccountsHistory to Invariants (Priority: Low)

**Current Status:** `check_consistency()` only checks `TransactionHashNumbers` and `StoragesHistory`. 

**Task:** Add `AccountsHistory` check following the same pattern as `StoragesHistory`.

**Location:** `crates/storage/provider/src/providers/rocksdb/invariants.rs`

```rust
// In check_consistency()
if provider.cached_storage_settings().accounts_history_in_rocksdb &&
    let Some(target) = self.check_accounts_history(provider)?
{
    unwind_target = Some(unwind_target.map_or(target, |t| t.min(target)));
}

fn check_accounts_history<Provider>(&self, provider: &Provider) -> ProviderResult<Option<BlockNumber>>
where
    Provider: DBProvider + StageCheckpointReader,
{
    // Same pattern as check_storages_history()
    let checkpoint = provider
        .get_stage_checkpoint(StageId::IndexAccountHistory)?
        .map(|cp| cp.block_number)
        .unwrap_or(0);
    
    // Check RocksDB data against checkpoint...
}
```

---

## File Change Summary

| File | Changes |
|------|---------|
| `crates/node/builder/src/launch/common.rs` | Add RocksDB consistency check + config validation |
| `crates/storage/provider/src/providers/rocksdb/invariants.rs` | Add `AccountsHistory` check |
| `crates/db-api/src/models.rs` | Add `PersistedRocksdbSettings` |
| `crates/storage/provider/src/traits/...` | Add settings persistence trait |
| `crates/stages/stages/src/stages/tx_lookup.rs` | Verify/add EitherWriter usage |
| `crates/stages/stages/src/stages/index_storage_history.rs` | Verify/add EitherWriter usage |
| `crates/stages/stages/src/stages/index_account_history.rs` | Verify/add EitherWriter usage |

---

## Testing Strategy

1. **Unit Tests** (in `invariants.rs`):
   - Test `AccountsHistory` invariant checks
   - Test combined unwind target calculation
   
2. **Integration Tests**:
   - Test startup with inconsistent RocksDB (should unwind)
   - Test startup with RocksDB ahead (should auto-prune)
   - Test config mismatch detection
   
3. **Manual Testing**:
   - Sync node with RocksDB enabled
   - Kill mid-sync, restart, verify consistency check runs
   - Try changing flags after sync, verify error

---

## Open Questions

1. **Unwind Coordination:** Should RocksDB and static file unwind targets be combined into a single unwind operation, or run separately?
   - **Recommendation:** Combine by taking minimum target

2. **Error vs Warning for Config Mismatch:** Hard error or warning?
   - **Recommendation:** Hard error (safer, prevents data corruption)

3. **Migration Path:** What if users want to enable RocksDB on existing MDBX-only nodes?
   - **Recommendation:** Document `reth db drop` + resync as the supported path. Future: add explicit migration command.

---

## Timeline Estimate

| Step | Effort | Dependencies |
|------|--------|--------------|
| Step 1: Consistency Check | M (1 day) | PR #20340 merged |
| Step 2: Stage Verification | S (0.5 day) | None |
| Step 3: Config Validation | M (1 day) | None |
| Step 4: AccountsHistory | S (0.5 day) | Step 1 |

**Total: ~3 days**
