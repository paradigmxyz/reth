---
title: Support for ERA (not ERA1) in stages sync
labels:
    - C-enhancement
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 16891
synced_at: 2026-01-21T11:32:15.989216Z
info:
    author: RomanHodulak
    created_at: 2025-06-18T13:35:40Z
    updated_at: 2025-07-12T15:40:16Z
---

### Describe the feature

Currently we support the ERA1 file format in the ERA stage of the stages sync pipeline.

Now the goal is to also support ERA file format, as described in the parent issue.

It probably makes the most sense to either extend the current `EraStage` or to rename it to `Era1Stage` and add a new one.

https://github.com/paradigmxyz/reth/blob/c3caea204701b8e8068a3c3535192e16875d1444/crates/stages/stages/src/stages/era.rs#L36

### Additional context

_No response_
