---
title: Wire up RocksDB initialization to Stages with EitherWriter
labels:
    - A-db
    - C-enhancement
    - inhouse
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20384
synced_at: 2026-01-21T11:32:16.013966Z
info:
    author: fgimenez
    created_at: 2025-12-15T17:48:38Z
    updated_at: 2026-01-21T11:03:54Z
---

Modify `IndexStorageHistoryStage` and `IndexAccountHistoryStage` to write history indices to RocksDB when the feature is enabled.

Both stages share [`load_history_indices`](https://github.com/paradigmxyz/reth/blob/main/crates/stages/stages/src/stages/utils.rs#L108) helper, so they should be updated together.

Contributes to RETH-112

### IndexStorageHistoryStage

In `crates/stages/stages/src/stages/index_storage_history.rs`:

1. [`execute()`](https://github.com/paradigmxyz/reth/blob/main/crates/stages/stages/src/stages/index_storage_history.rs#L58):
   - Currently uses `load_history_indices` which writes to `tables::StoragesHistory` via cursor
   - Change to: Use `EitherWriter::new_storages_history()` in the loading phase

2. [`unwind()`](https://github.com/paradigmxyz/reth/blob/main/crates/stages/stages/src/stages/index_storage_history.rs#L134):
   - Currently calls `provider.unwind_storage_history_indices_range()`
   - This delegates to `HistoryWriter::unwind_storage_history_indices_range()` 
   - May need additional changes if the provider method doesn't handle RocksDB (see #20388)

### IndexAccountHistoryStage

In `crates/stages/stages/src/stages/index_account_history.rs`:

1. [`execute()`](https://github.com/paradigmxyz/reth/blob/main/crates/stages/stages/src/stages/index_account_history.rs#L56):
   - Currently uses `load_history_indices` which writes to `tables::AccountsHistory` via cursor
   - Change to: Use `EitherWriter::new_account_history()` in the loading phase

2. [`unwind()`](https://github.com/paradigmxyz/reth/blob/main/crates/stages/stages/src/stages/index_account_history.rs#L130):
   - Currently calls `provider.unwind_account_history_indices_range()`
   - This delegates to `HistoryWriter::unwind_account_history_indices_range()` 
   - May need additional changes if the provider method doesn't handle RocksDB (see #20388)


Closes RETH-176
