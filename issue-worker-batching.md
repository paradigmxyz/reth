# perf: Batch proof tasks at worker level

## Problem

When proof tasks queue up in the worker pool (workers busy), each worker processes tasks one at a time. Multiple tasks for the same account result in redundant trie traversals.

The workers are the bottleneck (proof computation is slow), so tasks pile up in the **worker queue**, not the message channel.

## Current vs Proposed

```
CURRENT: No batching
──────────────────────────────────────────────
Worker receives Task1 → compute → receives Task2 → compute → receives Task3 → compute
= 3 proof computations

PROPOSED: Drain & batch at worker
──────────────────────────────────────────────
Worker receives Task1 → drain Task2, Task3 → merge → compute once
= 1 proof computation
```

## Solution

Apply the sparse trie drain pattern ([sparse_trie.rs#L81-L94](https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/payload_processor/sparse_trie.rs#L81-L94)) to proof workers.

Batch here, where workers receive from the queue:

- **Storage worker:** [proof_task.rs#L748-L749](https://github.com/paradigmxyz/reth/blob/main/crates/trie/parallel/src/proof_task.rs#L748-L749)
- **Account worker:** [proof_task.rs#L1028-L1029](https://github.com/paradigmxyz/reth/blob/main/crates/trie/parallel/src/proof_task.rs#L1028-L1029)

```rust
// Current: process one at a time
while let Ok(job) = work_rx.recv() {
    process(job);
}

// Proposed: drain & batch
while let Ok(first_job) = work_rx.recv() {
    let mut batch = vec![first_job];
    while let Ok(next) = work_rx.try_recv() {
        batch.push(next);  // Drain pending
    }
    process_batch(batch);  // Merge & compute once
}
```

## Note

This is complementary to #19168 (funnel-level batching). Funnel batching reduces tasks entering the queue; worker batching reduces tasks already queued.

## Impact

- Fewer proof computations when workers are backlogged
- Reduced redundant trie traversals for same account

## Related

- #19168 (funnel-level batching)
- #19166 (tracking: batch multiproof tasks)
