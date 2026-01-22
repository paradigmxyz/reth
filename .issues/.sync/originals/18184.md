---
title: 'perf: Reth Transaction Iterator Parallelization'
labels:
    - A-execution
    - C-enhancement
    - C-perf
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:15.997145Z
info:
    author: yongkangc
    created_at: 2025-09-01T08:54:25Z
    updated_at: 2025-09-01T09:22:30Z
---

### Problem
Sequential transaction processing delays prewarming cache in transaction pool. Iterator consumption at [crates/engine/tree/src/tree/payload_processor/mod.rs:273-280](https://github.com/paradigmxyz/reth/blob/yk/optimise_base_fee_allocation/crates/engine/tree/src/tree/payload_processor/mod.rs#L273-L280) creates bottleneck.


Current flow:
1. Iterator created early: [crates/optimism/evm/src/lib.rs#L287-L292](https://github.com/paradigmxyz/reth/blob/main/crates/optimism/evm/src/lib.rs#L287-L292)
2. Sequential consumption later: [crates/engine/tree/src/tree/payload_processor/mod.rs#L273-L280](https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/payload_processor/mod.rs#L273-L280)
3. Time gap wasted between creation of iter and consumption, so there could be some benefits with a rayon+spawn. 


### Proposed Change

ideally we convert it to a parallel one here https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/payload_processor/mod.rs#L273-L273

Replace sequential loop with parallel processing, and possibly create a rayon-style `par_bridge` wrapper for `ExecutableTxIterator`

The main challenge here is then probably yielding them in the correct order, or rather forwarding to regular execution in order. A possible soln would be to attach txs with an index and sorting them with a btree map.

### Additional context

### References
- Discussion: @mattsse identified bottleneck in sequential processing
- Similar to rayon's par_bridge: https://docs.rs/rayon/latest/rayon/iter/trait.ParallelBridge.html
- Current impl: https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/payload_processor/mod.rs#L273-L280
