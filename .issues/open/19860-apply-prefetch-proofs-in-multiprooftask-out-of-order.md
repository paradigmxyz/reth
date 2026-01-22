---
title: Apply prefetch proofs in `MultiProofTask` out of order
labels:
    - A-trie
    - C-perf
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:16.007192Z
info:
    author: shekhirin
    created_at: 2025-11-19T17:30:01Z
    updated_at: 2025-12-02T08:15:03Z
---

### Describe the feature

https://github.com/paradigmxyz/reth/blob/c57792cff4272e1fa5e0859b26b1f7c2389b6113/crates/engine/tree/src/tree/payload_processor/multiproof.rs#L731

Proof prefetch requests do not carry state updates, so when the proof is fetched, we can apply it straight away instead of assigning a sequence number to it and requiring no gaps https://github.com/paradigmxyz/reth/blob/c57792cff4272e1fa5e0859b26b1f7c2389b6113/crates/engine/tree/src/tree/payload_processor/multiproof.rs#L964-L981

### Additional context

_No response_
