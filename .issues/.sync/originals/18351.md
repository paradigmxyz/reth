---
title: Add history expiry to the transaction segment of the pruner
labels:
    - A-db
    - A-sdk
    - C-enhancement
assignees:
    - RomanHodulak
milestone: v1.11
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17230
synced_at: 2026-01-21T11:32:15.998287Z
info:
    author: RomanHodulak
    created_at: 2025-09-09T16:18:32Z
    updated_at: 2026-01-13T23:44:38Z
---

### Describe the feature

The pruner is triggered inside https://github.com/paradigmxyz/reth/blob/dd69dcbd012fb434cfacfbf9666ea7c5260d1ec4/crates/engine/tree/src/tree/mod.rs#L425-L452

If we integrate the history expiry based transaction pruning as a `Segment` of the `Pruner` then we will get them removed on a regular basis.

### Additional context

_No response_
