---
title: Move payload validation metric recording to `EngineValidator`
labels:
    - C-enhancement
    - S-needs-triage
assignees:
    - Timosdev99
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.996396Z
info:
    author: Rjected
    created_at: 2025-08-23T00:14:20Z
    updated_at: 2025-09-06T22:26:57Z
---

### Describe the feature

This no longer measures anything
https://github.com/paradigmxyz/reth/blob/a3298ecfdd2f604e6fc288bf803aa2980637e82d/crates/engine/tree/src/tree/mod.rs#L516-L518

This should be moved so that it records the part of the `EngineValidator` that does block validation:
https://github.com/paradigmxyz/reth/blob/a3298ecfdd2f604e6fc288bf803aa2980637e82d/crates/engine/tree/src/tree/payload_validator.rs#L445-L483

### Additional context

_No response_
