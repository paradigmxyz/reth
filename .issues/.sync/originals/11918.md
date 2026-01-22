---
title: Transaction is being written to both `DB` and `StaticFiles`
labels:
    - A-engine
    - C-bug
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.977571Z
info:
    author: joshieDo
    created_at: 2024-10-21T09:46:35Z
    updated_at: 2025-08-15T14:37:28Z
---

### Describe the feature

On the new engine we write to both unnecessarily

https://github.com/paradigmxyz/reth/blob/fbb27ebdad01780a2068399a01e7337bc0bb514f/crates/storage/provider/src/writer/mod.rs#L262

https://github.com/paradigmxyz/reth/blob/fbb27ebdad01780a2068399a01e7337bc0bb514f/crates/storage/provider/src/providers/database/provider.rs#L3410

The one on the `DatabaseProvider::insert_block` must be changed. However, we still support legacy engine, so it's not that straightforward as just removing it

### Additional context

_No response_
