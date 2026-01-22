---
title: Too much time is spent creating cursors in storage multiproofs
labels:
    - A-db
    - A-trie
    - C-perf
    - M-prevent-stale
    - S-needs-investigation
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:15.98389Z
info:
    author: shekhirin
    created_at: 2025-03-31T23:17:44Z
    updated_at: 2025-12-22T10:56:13Z
---

### Describe the feature

<img width="1279" alt="Image" src="https://github.com/user-attachments/assets/c1317f0c-55be-4c33-8779-a45de8980ccd" />

https://github.com/paradigmxyz/reth/blob/0a56694308344bc2f94b6cd9bed4a2d7ae39814b/crates/trie/db/src/hashed_cursor.rs#L42
https://github.com/paradigmxyz/reth/blob/0a56694308344bc2f94b6cd9bed4a2d7ae39814b/crates/storage/db/src/implementation/mdbx/tx.rs#L84-L95

### Additional context

_No response_
