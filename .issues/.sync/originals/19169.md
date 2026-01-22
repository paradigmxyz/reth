---
title: 'perf: Chunk and squash proof targets at account boundaries'
labels:
    - A-engine
    - C-enhancement
    - C-perf
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:16.000926Z
info:
    author: yongkangc
    created_at: 2025-10-21T03:53:15Z
    updated_at: 2025-10-21T03:53:15Z
---

### Describe the feature

Adjust the chunking logic so a single account’s storage slots stay together when we split work (chunk_size handling in both prefetch and state update paths) for better locality

[reth/crates/engine/tree/src/tree/payload_processor/multiproof.rs](https://github.com/paradigmxyz/reth/blob/e9598ba5ac4e32600e48b93d197a25603b1c644b/crates/engine/tree/src/tree/payload_processor/multiproof.rs#L738-L908)

basically, if the state updates arrive in the following order:

- Account A and its storage slot A
- Account A and its storage slot B
- Account A and its storage slot C

but we get to spawn only the first update before reaching the concurrency limit we have an opportunity to batch the storage slot B and C state updates into one multiproof, and then spawn it when concurrency limit allows us

### Additional context

_No response_
