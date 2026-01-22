---
title: Revisit static file indexes
labels:
    - A-static-files
    - C-debt
    - inhouse
assignees:
    - shekhirin
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 18682
synced_at: 2026-01-21T11:32:16.006866Z
info:
    author: shekhirin
    created_at: 2025-11-17T17:47:43Z
    updated_at: 2025-11-24T13:42:08Z
---

### Describe the feature

We need to revisit these indexes https://github.com/paradigmxyz/reth/blob/1568f4c45179aa3b86fe80a2934d9e23eddddbed/crates/storage/provider/src/providers/static_file/manager.rs#L289-L324

Some things are confusing, for example why we store `SegmentRangeInclusive` for `static_files_min_block`, but just a `u64` for `static_files_max_block`?

### Additional context

_No response_
