---
title: Flatten HashedPostState before persisting
labels:
    - A-db
    - C-enhancement
    - C-perf
    - M-prevent-stale
    - S-stale
type: Task
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20607
synced_at: 2026-01-21T11:32:16.016489Z
info:
    author: mediocregopher
    created_at: 2025-12-23T15:18:59Z
    updated_at: 2026-01-21T11:16:42Z
---

### Describe the feature

When persisting hashed state we do so block-by-block:
https://github.com/paradigmxyz/reth/blob/b79c58d83537b8db52a6a6c02e81d2f868bd4f6d/crates/storage/provider/src/providers/database/provider.rs#L355-L370

We could instead accumulate state into a single HashedPostState and persist it all at once.

### Additional context

_No response_

Closes RETH-98
