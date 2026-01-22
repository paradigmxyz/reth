---
title: Close engine thread on shutdown
labels:
    - C-enhancement
    - inhouse
assignees:
    - mattsse
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.007032Z
info:
    author: mattsse
    created_at: 2025-11-19T12:27:11Z
    updated_at: 2025-11-19T16:32:51Z
---

### Describe the feature

the engine task

https://github.com/paradigmxyz/reth/blob/e93bd0a087108c4e46d06ee61343e708d591bb19/crates/engine/tree/src/tree/mod.rs#L340-L340

has an entire channel, so this will never close on its own. this is fine because it is intended to run for the entire duration of the program but for testing a full tear down would be beneficial.
for this we'd need to trigger a close command or similar via:

https://github.com/paradigmxyz/reth/blob/e93bd0a087108c4e46d06ee61343e708d591bb19/crates/engine/tree/src/engine.rs#L173-L175

### Additional context

_No response_
