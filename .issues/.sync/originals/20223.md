---
title: 'perf: batch consecutive transaction state updates in multiproof task'
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:16.01138Z
info:
    author: yongkangc
    created_at: 2025-12-09T11:14:44Z
    updated_at: 2025-12-10T03:56:54Z
---

## Problem

Currently, the multiproof task's state update batching is overly restrictive. The `can_batch_state_update` function only merges updates from the **same** `StateChangeSource`:

```rust
fn same_state_change_source(lhs: StateChangeSource, rhs: StateChangeSource) -> bool {
    match (lhs, rhs) {
        (StateChangeSource::Transaction(a), StateChangeSource::Transaction(b)) => a == b,
        (StateChangeSource::PreBlock(a), StateChangeSource::PreBlock(b)) => {
            mem::discriminant(&a) == mem::discriminant(&b)
        }
        (StateChangeSource::PostBlock(a), StateChangeSource::PostBlock(b)) => {
            mem::discriminant(&a) == mem::discriminant(&b)
        }
        _ => false,
    }
}
```

This means:
- Transaction updates **never** batch together (each tx has unique ID)
- PreBlock/PostBlock only batch with same discriminant
- Cross-type batching (e.g., PreBlock + Transaction) is blocked

## Solution

Remove `can_batch_state_update` entirely. All state updates (PreBlock, Transaction, PostBlock) arrive at the MultiProofTask in correct execution order, so they can all be safely batched together regardless of source type.

```rust
// Before: complex source matching
if !can_batch_state_update(batch_source, batch_update, next_source, &next_update) {
    break;
}

// After: batch everything
// (just remove the check entirely)
```

Another possible thing that we could do would be 
for prefetch we don't need the sequencer, we can just forward them directly to the the sparse trie task without sequencing. That would save us one hop. See comment here
https://github.com/paradigmxyz/reth/pull/20066#discussion_r2601985018

## Rationale

The `ProofSequencer` already ensures correct ordering when applying updates to the sparse trie. The batching at the message level is purely an optimization to reduce proof computation overhead - it doesn't affect correctness.

## Expected Impact

- Reduced number of proof computations by batching consecutive state updates
- Better amortization of multiproof work during block execution
- Simpler code with fewer edge cases
