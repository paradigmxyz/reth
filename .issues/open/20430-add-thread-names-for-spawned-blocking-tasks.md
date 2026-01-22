---
title: Add thread names for spawned blocking tasks
labels:
    - A-engine
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.014604Z
info:
    author: shekhirin
    created_at: 2025-12-16T19:48:30Z
    updated_at: 2026-01-09T08:30:19Z
---

Several blocking tasks are spawned without descriptive thread names, making it harder to identify them in profilers like Samply and debugging tools.

### Locations

- [`crates/engine/tree/src/tree/payload_processor/mod.rs:446`](https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/payload_processor/mod.rs#L446) - prewarm task spawn
- [`crates/engine/tree/src/tree/payload_processor/prewarm.rs:148`](https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/payload_processor/prewarm.rs#L148) - prewarm executor spawn
- [`crates/trie/parallel/src/proof_task.rs:153`](https://github.com/paradigmxyz/reth/blob/main/crates/trie/parallel/src/proof_task.rs#L153) - proof task worker threads

### Suggested Fix

Add descriptive names to `spawn_blocking` calls so threads can be easily identified during profiling sessions.
