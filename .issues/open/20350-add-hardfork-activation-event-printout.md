---
title: Add hardfork activation event printout
labels:
    - A-observability
    - C-enhancement
    - D-good-first-issue
assignees:
    - futreall
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.013644Z
info:
    author: mattsse
    created_at: 2025-12-13T08:49:09Z
    updated_at: 2025-12-13T08:50:54Z
---

### Describe the feature

we emit events on new blocks

https://github.com/paradigmxyz/reth/blob/3380eb69c87a57b761efde62a10b459eb61b5020/crates/engine/primitives/src/event.rs#L31-L32

we can give this function access to `box<dyn Hardforks>`

https://github.com/paradigmxyz/reth/blob/3380eb69c87a57b761efde62a10b459eb61b5020/crates/ethereum/hardforks/src/hardforks/mod.rs#L10-L18


https://github.com/paradigmxyz/reth/blob/3380eb69c87a57b761efde62a10b459eb61b5020/crates/node/events/src/node.rs#L367-L367 

then we need to track if a new hardfork activated, which can be done by adding a helper that checks if any forkcondition has the exact same timestamp or rather the first time we transition the one

### Additional context

_No response_
