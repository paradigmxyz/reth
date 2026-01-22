---
title: Use changeset-based pruning for StoragesHistory and AccountHistory in RocksDB
labels:
    - A-db
    - C-enhancement
    - S-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.014127Z
info:
    author: fgimenez
    created_at: 2025-12-16T12:56:42Z
    updated_at: 2026-01-21T02:17:06Z
---

### Describe the feature

Currently, `RocksDBProvider::prune_storages_history_above` iterates all rows to find entries to delete, which is not scalable for large tables.

The proper approach would be to use `StorageChangeSets` to determine which keys need to be pruned, similar to how the pruner in `crates/prune/prune/src/segments/user/storage_history.rs` works, and how `TransactionHashNumbers` pruning fetches transactions by range.

`StorageChangeSets` are currently stored in MDBX, not static files. When the checkpoint is behind, the changesets are also behind, so we can't use them to determine which keys to prune. Once `StorageChangeSets` are moved to static files, refactor to use the changeset-based approach.


### Additional context

_No response_
