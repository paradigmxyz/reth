---
title: Avoid creating RocksDB transactions for legacy MDBX-only nodes
labels:
    - A-db
    - A-rocksdb
    - C-enhancement
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20384
synced_at: 2026-01-21T11:32:16.01978Z
info:
    author: yongkangc
    created_at: 2026-01-16T21:32:16Z
    updated_at: 2026-01-21T10:59:01Z
---

## Summary

`with_rocksdb_tx` always creates a RocksDB transaction via `rocksdb_provider.tx()` even for legacy MDBX-only nodes where RocksDB isn't used for history indices. This adds unnecessary overhead.

## Problem

When `EitherReader::new_*` constructors are called, they receive a `RocksTxRefArg` and check `storage_settings` to decide whether to use RocksDB. On legacy nodes, these flags are `false`, so the tx is created but never used.

## Proposed Solution

Make `RocksTxRefArg` an `Option<&RocksTx>` and check `storage_settings.any_history_in_rocksdb()` in `with_rocksdb_tx` **before** creating the transaction:

```rust
fn with_rocksdb_tx<F, R>(&self, f: F) -> ProviderResult<R> {
    if !self.cached_storage_settings().any_history_in_rocksdb() {
        return f(None);  // No tx created
    }
    let tx = self.rocksdb_provider().tx();
    f(Some(&tx))
}
```

## Related

Closes RETH-136

Part of #20384
