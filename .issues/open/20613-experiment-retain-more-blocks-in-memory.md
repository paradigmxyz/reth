---
title: 'Experiment: Retain more blocks in memory'
labels:
    - A-db
    - C-enhancement
    - C-perf
    - inhouse
type: Task
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20607
synced_at: 2026-01-21T11:32:16.017102Z
info:
    author: mediocregopher
    created_at: 2025-12-23T15:48:16Z
    updated_at: 2026-01-07T12:14:29Z
---

### Describe the feature

We should experiment with retaining more blocks in memory prior to flushing them to disk.

Currently we have `--engine.persistence-threshold` default to 2 blocks, and `--engine.memory-block-buffer-target` is ignored. This means every 2 blocks we flush _all_ blocks to disk.

Some things worth trying:

* Increasing the persistence threshold to a higher limit, like 6 or 10.

* Re-introducing the memory block buffer target at some intermediate value, like 2. This would mean we always have at least 2 blocks in memory. With this in place we would have to pull reverts from disk significantly less often.

### Additional context

_No response_
