---
title: Re-use cursors in Latest/HistoricalStateProvider
labels:
    - A-db
    - C-enhancement
    - C-perf
assignees:
    - mediocregopher
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:16.01554Z
info:
    author: mediocregopher
    created_at: 2025-12-18T14:34:48Z
    updated_at: 2025-12-18T14:35:06Z
---

### Describe the feature

Every time we read a storage slot, we create a new dup cursor only to read it one storage slot. This behaviour comes from the fact that duplicate keys in dupsort tables can't be read with a simple mdbx_get.

In Latest/HistoricalStateProvider we should cache cursors for the history tables and plain state tables which are created on the provider itself, so they can be re-used in subsequent calls to the same provider.

### Additional context

_No response_
