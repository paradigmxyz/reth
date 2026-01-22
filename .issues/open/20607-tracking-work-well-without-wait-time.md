---
title: 'Tracking: Work well without wait-time'
labels:
    - C-enhancement
    - C-perf
    - C-tracking-issue
assignees:
    - mediocregopher
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.016226Z
info:
    author: mediocregopher
    created_at: 2025-12-23T15:09:09Z
    updated_at: 2025-12-23T15:09:29Z
---

### Describe the feature

reth does not currently function well without some kind of wait time between nP+FCU calls. During our own benching we generally use something like 400ms, but ideally we eliminate this requirement.

When run without any wait time between blocks, we see blocks pile up in memory faster than they get flushed to disk. This is almost certainly a bug which needs to be fixed, as without doing so reth becomes unusable.

There are other improvements we can make in this area as well:

* There are some low-hanging persistence optimizations which can be made.
* We can investigate increasing number of blocks which are cached in memory.
* Future-proof by benching with larger blocks

### Additional context

_No response_
