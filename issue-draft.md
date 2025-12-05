# perf: Batch multiproof tasks at MultiProofTask (funnel level)

## Problem

When state updates arrive faster than workers can compute proofs, tasks queue up but are **not batched**. Each task is processed separately, resulting in redundant trie traversals.

We could batch at the worker level, but workers only see a fraction of total work (1/N each). `MultiProofTask` is the central funnel where **all** targets pass through before distribution—batching here yields larger batches.

## Current vs Proposed

```
CURRENT: No batching
──────────────────────────────────────────────
10 updates → dispatch 10 tasks → 10 proof computations

PROPOSED: Batch at funnel (before distribution)
──────────────────────────────────────────────
10 updates → drain & merge → dispatch 1 task → 1 proof computation
```

## Solution

Apply the sparse trie drain pattern ([sparse_trie.rs#L81-L94](https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/payload_processor/sparse_trie.rs#L81-L94)) to `MultiProofTask`:

```rust
while let Ok(mut update) = rx.recv() {
    while let Ok(next) = rx.try_recv() {
        update.extend(next);  // Drain & merge
    }
    dispatch(update);  // Single batch
}
```

**File:** `crates/engine/tree/src/tree/payload_processor/multiproof.rs`

## Impact

- Fewer proof computations under load
- Larger batches = better amortization of trie traversal costs

## Related

- #19168, #19166
