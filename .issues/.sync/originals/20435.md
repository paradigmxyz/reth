---
title: Deduplicate state_provider_builder calls in payload validator
labels:
    - A-engine
    - C-perf
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.01507Z
info:
    author: shekhirin
    created_at: 2025-12-16T20:04:19Z
    updated_at: 2026-01-02T03:39:54Z
---

## Summary

There are two implementations of `state_provider_builder` that could be deduplicated.

https://github.com/paradigmxyz/reth/blob/4231f4b68879d673ebab41b8a7e434537e6c6f0c/crates/engine/tree/src/tree/mod.rs#L2483

https://github.com/paradigmxyz/reth/blob/4231f4b68879d673ebab41b8a7e434537e6c6f0c/crates/engine/tree/src/tree/payload_validator.rs#L365

## Solution

Instead of discarding the `state_provider_builder` here https://github.com/paradigmxyz/reth/blob/4231f4b68879d673ebab41b8a7e434537e6c6f0c/crates/engine/tree/src/tree/mod.rs#L2483 pass it as the argument to `execute` closure inside the `ctx` https://github.com/paradigmxyz/reth/blob/4231f4b68879d673ebab41b8a7e434537e6c6f0c/crates/engine/tree/src/tree/mod.rs#L2523 to not create it again here https://github.com/paradigmxyz/reth/blob/4231f4b68879d673ebab41b8a7e434537e6c6f0c/crates/engine/tree/src/tree/payload_validator.rs#L364-L365
