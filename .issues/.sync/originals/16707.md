---
title: Payload builder changes to support millisecond blocks
labels:
    - A-block-building
    - C-bug
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.988409Z
info:
    author: frisitano
    created_at: 2025-06-06T18:14:52Z
    updated_at: 2025-07-15T09:39:48Z
---

### Describe the feature

The current payload builder components do not incorporate any concept of block time. As a result, when a large number of transactions are present in the pool, the builder aggressively attempts to process and include all available transactions before sealing a block.

This behavior works fine under standard 12-second slot durations, where there's ample time to process the mempool. However, it breaks down in environments with millisecond-scale block times — such as those we use at Scroll — where the transaction processing loop becomes a bottleneck and leads to unstable or delayed block production.

At Scroll, we will probably address this by introducing a timeout mechanism in the `ScrollPayloadBuilder` transaction processing loop (see below). A similar mechanism — or a more general abstraction at the payload service level — could be beneficial for Ethereum and Optimism builders as well.

Suggestion: Consider implementing a transaction loop timeout or sealing deadline mechanism in the payload builder to ensure more predictable block times under high load.

https://github.com/paradigmxyz/reth/blob/1e277921c783cd0e49ce53cd5dbb28c062702cf1/crates/ethereum/payload/src/lib.rs#L200-L218

### Additional context

_No response_
