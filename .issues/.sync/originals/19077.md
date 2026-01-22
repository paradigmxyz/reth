---
title: 'perf(storage): optimize wiped storage entry collection to avoid unnecessary Vec allocation'
labels:
    - A-db
    - C-perf
    - D-good-first-issue
assignees:
    - WilliamNwoke
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 19057
synced_at: 2026-01-21T11:32:16.000639Z
info:
    author: yongkangc
    created_at: 2025-10-16T11:20:11Z
    updated_at: 2025-10-16T13:50:00Z
---

### Problem
There's a [TODO comment at provider.rs:2279-2287](https://github.com/paradigmxyz/reth/blob/386eaa3ff68b5e05bfa64d76837a676baf3582cc/crates/storage/provider/src/providers/database/provider.rs#L2279-L2287) indicating that the collection of wiped storage entries could be rewritten more efficiently:

```rust
// TODO(mediocregopher): This could be rewritten in a way which doesn't require
// collecting wiped entries into a Vec like this, see
// `write_storage_trie_changesets`.
let mut wiped_storage = Vec::new();
if wiped {
    tracing::trace!(?address, "Wiping storage");
    if let Some((_, entry)) = storages_cursor.seek_exact(address)? {
        wiped_storage.push((entry.key, entry.value));
        while let Some(entry) = storages_cursor.next_dup_val()? {
            wiped_storage.push((entry.key, entry.value))
        }
    }
}
```

Currently, the code:
1. Allocates a Vec to collect all wiped storage entries
2. Iterates through the cursor and pushes each entry into the Vec
3. This intermediate collection adds allocation overhead and an extra iteration

### Proposed Optimization
Refactor to use the approach from `write_storage_trie_changesets` which likely:
- Processes entries directly from the cursor without intermediate collection
- Avoids unnecessary Vec allocations
- Reduces memory pressure during block persistence

### Context
This optimization is part of the broader [static file write optimization effort (#19057)](https://github.com/paradigmxyz/reth/issues/19057) to reduce block persistence latency and ensure write throughput matches block production rates.
