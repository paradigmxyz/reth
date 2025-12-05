# Plan: Batch Multiproof Tasks at Worker Pool Level

**Issue**: https://github.com/paradigmxyz/reth/issues/19168
**Goal**: When multiple proof tasks queue up in the worker pool, drain and merge them before processing (like sparse_trie.rs does).

## Problem

Currently, workers process ONE job at a time:
```rust
while let Ok(job) = work_rx.recv() {  // Get ONE job
    process(job);                      // Process it
}
```

When workers are busy, tasks queue up separately. When a worker becomes free, it picks ONE task instead of merging all pending tasks.

**Optimization opportunity**: If 3 storage proof requests for Account A are queued, merge them into ONE multiproof computation.

## Solution: Drain-and-Merge Pattern

Apply the sparse_trie.rs pattern to proof workers:
```rust
while let Ok(first_job) = work_rx.recv() {
    let mut batch = vec![first_job];
    while let Ok(next) = work_rx.try_recv() {  // Drain pending
        batch.push(next);                       // Collect
    }
    process_batch(batch);                       // Process merged
}
```

## Key Challenge: Sequence Numbers

Each job has a unique `sequence_number` for `ProofSequencer` ordering. If we merge jobs 5,6,7 into one batch:
- We get ONE result, but sequencer expects THREE

**Solution**: Send multiple results back:
- First sequence (5): Full merged proof + its state
- Remaining (6,7): Empty proofs + their original states

The empty proofs pass through `ProofSequencer` and `SparseTrieUpdate::extend()` handles merging correctly.

## Implementation

### File: `crates/trie/parallel/src/proof_task.rs`

### Step 1: Modify `StorageProofWorker::run()` (lines 749-779)

**From:**
```rust
while let Ok(job) = work_rx.recv() {
    available_workers.fetch_sub(1, Ordering::Relaxed);
    match job {
        StorageWorkerJob::StorageProof { input, proof_result_sender } => {
            Self::process_storage_proof(...);
        }
        StorageWorkerJob::BlindedStorageNode { ... } => { ... }
    }
    available_workers.fetch_add(1, Ordering::Relaxed);
}
```

**To:**
```rust
while let Ok(first_job) = work_rx.recv() {
    available_workers.fetch_sub(1, Ordering::Relaxed);

    match first_job {
        StorageWorkerJob::StorageProof { input, proof_result_sender } => {
            let target_address = input.hashed_address;
            let mut batch = vec![(input, proof_result_sender)];

            // Drain pending jobs for SAME address (up to limit)
            while batch.len() < MAX_STORAGE_BATCH_SIZE {
                match work_rx.try_recv() {
                    Ok(StorageWorkerJob::StorageProof { input: next, proof_result_sender: ctx })
                        if next.hashed_address == target_address =>
                    {
                        batch.push((next, ctx));
                    }
                    Ok(other) => {
                        // Different job - process batch, then handle other
                        Self::process_storage_batch(&proof_tx, batch, ...);
                        batch = vec![];
                        Self::handle_job(other, ...);
                        break;
                    }
                    Err(_) => break,
                }
            }

            if !batch.is_empty() {
                Self::process_storage_batch(&proof_tx, batch, ...);
            }
        }
        StorageWorkerJob::BlindedStorageNode { ... } => { /* unchanged */ }
    }

    available_workers.fetch_add(1, Ordering::Relaxed);
}
```

### Step 2: Add `process_storage_batch()` method

```rust
fn process_storage_batch(
    proof_tx: &ProofTaskTx<Provider>,
    jobs: Vec<(StorageProofInput, ProofResultContext)>,
    ...
) {
    if jobs.len() == 1 {
        // Fast path: single job
        Self::process_storage_proof(...);
        return;
    }

    // Merge inputs
    let mut merged_slots = B256Set::default();
    let mut merged_prefix = PrefixSetMut::default();
    let mut contexts = Vec::new();
    let hashed_address = jobs[0].0.hashed_address;

    for (input, ctx) in jobs {
        merged_slots.extend(input.target_slots);
        merged_prefix.extend(input.prefix_set.iter());
        contexts.push(ctx);
    }

    // Compute ONE proof
    let result = compute_storage_proof(hashed_address, merged_slots, merged_prefix);

    // Send result for FIRST sequence (full proof)
    let first = contexts.remove(0);
    first.sender.send(ProofResultMessage {
        sequence_number: first.sequence_number,
        result: result.clone(),
        state: first.state,
        ...
    });

    // Send empty proofs for remaining sequences
    for ctx in contexts {
        ctx.sender.send(ProofResultMessage {
            sequence_number: ctx.sequence_number,
            result: Ok(empty_storage_proof()),
            state: ctx.state,
            ...
        });
    }
}
```

### Step 3: Modify `AccountProofWorker::run()` (lines 1029-1058)

Same pattern, but batch all `AccountMultiproof` jobs (not just same-address):

```rust
while let Ok(first_job) = work_rx.recv() {
    available_workers.fetch_sub(1, Ordering::Relaxed);

    match first_job {
        AccountWorkerJob::AccountMultiproof { input } => {
            let mut batch = vec![*input];

            while batch.len() < MAX_ACCOUNT_BATCH_SIZE {
                match work_rx.try_recv() {
                    Ok(AccountWorkerJob::AccountMultiproof { input: next }) => {
                        batch.push(*next);
                    }
                    Ok(other) => {
                        Self::process_account_batch(..., batch, ...);
                        batch = vec![];
                        Self::handle_job(other, ...);
                        break;
                    }
                    Err(_) => break,
                }
            }

            if !batch.is_empty() {
                Self::process_account_batch(..., batch, ...);
            }
        }
        AccountWorkerJob::BlindedAccountNode { ... } => { /* unchanged */ }
    }

    available_workers.fetch_add(1, Ordering::Relaxed);
}
```

### Step 4: Add `process_account_batch()` method

```rust
fn process_account_batch(
    proof_tx: &ProofTaskTx<Provider>,
    storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    inputs: Vec<AccountMultiproofInput>,
    ...
) {
    if inputs.len() == 1 {
        Self::process_account_multiproof(...);
        return;
    }

    // Merge targets and prefix_sets
    let mut merged_targets = MultiProofTargets::default();
    let mut merged_prefix_sets = TriePrefixSets::default();
    let mut contexts = Vec::new();

    for input in inputs {
        merged_targets.extend(input.targets);
        // merge prefix_sets...
        contexts.push(input.proof_result_sender);
    }

    // Compute ONE multiproof
    let result = compute_account_multiproof(merged_targets, merged_prefix_sets, ...);

    // Send full result for first, empty for rest
    // (same pattern as storage batch)
}
```

### Constants to Add

```rust
const MAX_STORAGE_BATCH_SIZE: usize = 16;
const MAX_ACCOUNT_BATCH_SIZE: usize = 8;
```

## Files to Modify

| File | Changes |
|------|---------|
| `crates/trie/parallel/src/proof_task.rs` | Worker run loops + batch methods |

## Testing

1. Unit tests for batch logic (1, 2, N jobs)
2. Verify all sequence numbers receive responses
3. Verify ordering preserved through `ProofSequencer`
4. Benchmark improvement under load

## Expected Impact

- Fewer DB cursor traversals when multiple proofs target same account
- Better worker utilization under high load
- No changes needed to `MultiProofTask`, `ProofSequencer`, or `SparseTrieTask`
