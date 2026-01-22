---
title: 'RPC: Batch tx submissions to insert into the mempool together'
labels:
    - C-discussion
    - C-enhancement
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.979055Z
info:
    author: hai-rise
    created_at: 2024-11-23T16:17:58Z
    updated_at: 2025-07-02T10:14:36Z
---

### Describe the feature

Currently, the RPC server submits each transaction individually which competes for a write lock of the pool. This can be very congested for performance chains that expect 100k+ new transactions every second. Even more challenging if the block time is small -- canonical state changes also compete for the write lock several times a second.

Ideally, the RPC server would buffer transactions to insert in a batch when the pool write lock is unavailable, instead of competing for the write lock (and block anyway) for every insertion request.



### Additional context

Related: #12806.
