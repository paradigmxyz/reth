---
title: Add docs about storage consistency when using `ProviderFactory` writers as a library
labels:
    - A-db
    - A-rocksdb
    - A-static-files
    - C-docs
    - inhouse
assignees:
    - joshieDo
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.018186Z
info:
    author: joshieDo
    created_at: 2026-01-09T21:07:09Z
    updated_at: 2026-01-20T12:41:15Z
---

### Describe the change

When a node crashes or has an ungraceful shutdown, we heal any inconsistencies in node builder:
https://github.com/paradigmxyz/reth/blob/52c2ae33626b06086555f90fe32f3978d14f8c08/crates/node/builder/src/launch/common.rs#L461-L469


`ProviderFactory` being generic over tables and data types doesn't by itself provide ways to accomplish the above, and that's why we do it on node builder. However, anyone using `ProviderFactory` writers directly (without node builder), might not be aware of these nuances and be working on an inconsistent state. We should at least add some documentation alluding to how consistency checks/heals should be made, by looking at the node builder and our provider commits.

Refs:
* https://github.com/paradigmxyz/reth/blob/b1d75f2771439d8ab80f5a5fc6dbde9fb42478b9/crates/storage/provider/src/providers/database/provider.rs#L3424-L3457
* https://github.com/paradigmxyz/reth/blob/b1d75f2771439d8ab80f5a5fc6dbde9fb42478b9/crates/storage/provider/src/providers/static_file/manager.rs#L1157-L1192
* https://github.com/paradigmxyz/reth/blob/b1d75f2771439d8ab80f5a5fc6dbde9fb42478b9/crates/storage/provider/src/providers/static_file/manager.rs#L1427-L1429

### Additional context

_No response_
