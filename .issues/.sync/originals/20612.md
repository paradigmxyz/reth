---
title: 'Experiment: flatten plain state for MemoryStateProvider'
labels:
    - A-engine
    - C-enhancement
    - C-perf
assignees:
    - Rustix69
type: Task
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20607
synced_at: 2026-01-21T11:32:16.01664Z
info:
    author: mediocregopher
    created_at: 2025-12-23T15:42:21Z
    updated_at: 2026-01-18T09:14:07Z
---

### Describe the feature

When using MemoryStateProvider we pass it an array of `ExecutedBlock`s, which it then iterates over to serve requests:

https://github.com/paradigmxyz/reth/blob/b79c58d83537b8db52a6a6c02e81d2f868bd4f6d/crates/chain-state/src/memory_overlay.rs#L111-L115

https://github.com/paradigmxyz/reth/blob/b79c58d83537b8db52a6a6c02e81d2f868bd4f6d/crates/chain-state/src/memory_overlay.rs#L215-L219

https://github.com/paradigmxyz/reth/blob/b79c58d83537b8db52a6a6c02e81d2f868bd4f6d/crates/chain-state/src/memory_overlay.rs#L227-L231

We should see if we can flatten the BundleState of the `ExecutedBlock`s into a single state, which can be applied as an overlay instead. This can be done asynchronously in a similar manner to the trie input:

https://github.com/paradigmxyz/reth/blob/b79c58d83537b8db52a6a6c02e81d2f868bd4f6d/crates/engine/tree/src/tree/payload_validator.rs#L1050

Note that we can only use this flattened state if its anchor block shares the anchor of the block being validated. If not then we must either compute a new flattened state or fallback to the current behavior.

### Additional context

_No response_
