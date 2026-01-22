---
title: Add metrics for `ForkchoiceStateTracker` state
labels:
    - C-enhancement
    - S-needs-triage
assignees:
    - figtracer
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.017424Z
info:
    author: Rjected
    created_at: 2025-12-23T18:22:28Z
    updated_at: 2025-12-23T19:17:58Z
---

### Describe the feature

Right now we have the `ForkchoiceStateTracker` which keeps track of the latest FCUs received by the engine API. We should track these in metrics, so that we can see what the latest valid state is without logs


https://github.com/paradigmxyz/reth/blob/29438631be7b6492ac5e5c3cb5202b78e8792059/crates/engine/primitives/src/forkchoice.rs#L4-L15


### Additional context

_No response_
