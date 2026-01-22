---
title: Recover senders for imported network transactions in a blocking task
labels:
    - A-tx-pool
    - C-perf
assignees:
    - klkvr
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.012544Z
info:
    author: shekhirin
    created_at: 2025-12-10T15:23:47Z
    updated_at: 2025-12-10T21:25:45Z
---

### Describe the feature

When many network transactions are imported at once, this snippet becomes a bottleneck

https://github.com/paradigmxyz/reth/blob/b7d881510422ac03340a63e8c08239950e8226b0/crates/net/network/src/transactions/mod.rs#L1376

This is problematic for two reasons:
1. Senders are recovered sequentially.
2. This is done in an async task, and with many import tasks happening simultaneously, can block the runtime for other async tasks.

We should instead recover the senders in parallel in a separate blocking task.

### Additional context

_No response_
