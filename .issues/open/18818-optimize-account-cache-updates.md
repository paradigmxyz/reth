---
title: Optimize account cache updates
labels:
    - C-enhancement
    - C-perf
    - D-good-first-issue
assignees:
    - futreall
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.999985Z
info:
    author: mattsse
    created_at: 2025-10-01T14:45:23Z
    updated_at: 2025-12-03T18:10:13Z
---

### Describe the feature

This 

https://github.com/paradigmxyz/reth/blob/a2bde852bb1480b7e3a5853524cf82eec677b042/crates/engine/tree/src/tree/cached_state.rs#L398-L403

loops over:
https://github.com/paradigmxyz/reth/blob/a2bde852bb1480b7e3a5853524cf82eec677b042/crates/engine/tree/src/tree/cached_state.rs#L325-L338


which we could optimize to only do the account lookup once:
https://github.com/paradigmxyz/reth/blob/a2bde852bb1480b7e3a5853524cf82eec677b042/crates/engine/tree/src/tree/cached_state.rs#L332-L337

### Additional context

_No response_
