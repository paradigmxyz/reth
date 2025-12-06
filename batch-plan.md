# Plan: Batch Multiproof Tasks at MultiProofTask Message Loop

**Issue:** https://github.com/paradigmxyz/reth/issues/19168

## Problem

When state updates arrive faster than workers can process them, each task is dispatched individually with often just 1 target. This leads to many separate multiproof computations, causing redundant trie traversals and lower worker efficiency.

**Current state**: Most multiproofs have just 1 target
**Target**: ~10 targets per multiproof
**Max batch size**: 50 targets (stop draining when limit reached, counted as accounts + storage slots + wipes)

## Objective

Merge pending multiproof requests in the message queue before dispatching, so we schedule fewer separate multiproofs with more targets each.

## Success Metrics

**Existing proof-size metrics (should trend higher):**
- `state_update_proof_targets_accounts_histogram`
- `state_update_proof_targets_storages_histogram`

**New batching metrics:**
- `batched_state_update_messages_histogram` - count of `StateUpdate` messages per drain
- `batched_state_update_targets_histogram` - total targets (accounts + storage slots + wipes) in merged state per drain
- `batched_prefetch_messages_histogram` - count of `PrefetchProofs` messages per drain

**Notes:**
- `state_updates_received_histogram` tracks dispatched proofs (not raw messages), so rely on the above batching metrics plus the existing target histograms to validate improvement.
- Expect `proofs_processed_histogram` to remain close to dispatched proof count; batching should reduce dispatches for the same work.

## Ordering Considerations

**PrefetchProofs**: Order doesn't matter - these are prefetches with no sequencing requirements.

**StateUpdates**: Ordering is preserved because:
1. Messages are received in order and accumulated
2. States are merged via `HashedPostState::extend()` (later update wins)
3. Sequence numbers are assigned at dispatch time via `proof_sequencer.next_sequence()`
4. `ProofSequencer` ensures results are delivered to sparse trie in order

The key insight: **ordering is about sequence numbers at dispatch, not message receipt order**. By batching before dispatch, we aggregate state while preserving the sequencing guarantee.

## Solution

Apply the batching pattern from `sparse_trie.rs` to the `MultiProofTask` message loop: after a blocking `recv`, drain pending messages with `try_recv`, merge them while respecting a target budget, then dispatch the aggregated work.

## Algorithm TL;DR & Rationale

- Block on `rx.recv()` for the first control message (this is "block on rx": use the blocking receive so the thread parks until work arrives) to avoid busy-spinning; once we have one message, immediately drain with `try_recv` to scoop up the queued work into the same batch.
- Maintain a `finished` flag so that a `FinishedStateUpdates` message that arrives inside a drain loop is not lost; we still process any earlier updates in the same batch, then exit when `is_done` becomes true.
- Batch budget is based on the **combined chunking length of the merged batches** (prefetch + state updates, counting wipes) so duplicates don't prematurely stop the drain.
- Process order per batch: `EmptyProof` messages first (sequence numbers already assigned), then merged `PrefetchProofs`, then merged `StateUpdate`s, record batching metrics, finally mark finished and run `is_done`.

## Files to Modify

- `crates/engine/tree/src/tree/payload_processor/multiproof.rs`

## Implementation Steps

### Step 1: Add New Metrics (lines ~507-570)

```rust
/// Histogram of StateUpdate messages batched together
pub batched_state_update_messages_histogram: Histogram,
/// Histogram of total targets (accounts + storage slots + wipes) in a merged StateUpdate batch
pub batched_state_update_targets_histogram: Histogram,
/// Histogram of PrefetchProofs messages batched together
pub batched_prefetch_messages_histogram: Histogram,
```

### Step 2: Add Batch Constants

```rust
/// Maximum number of targets to batch before stopping the drain loop
const MAX_BATCH_TARGETS: usize = 50;
```

### Step 3: Create Batch Collection Helper

Add a helper method that categorizes messages and **returns the combined target count in the batch** (deduped across already-merged messages):

```rust
/// Returns total targets in the current batch after merging this message.
fn add_to_batch(
    msg: MultiProofMessage,
    prefetch_batch: &mut MultiProofTargets,
    state_update_batch: &mut HashedPostState,
    empty_proof_batch: &mut Vec<(u64, HashedPostState)>,
    finished: &mut bool,
) -> usize {
    match msg {
        MultiProofMessage::PrefetchProofs(targets) => {
            prefetch_batch.extend(targets);
        }
        MultiProofMessage::StateUpdate(_source, update) => {
            state_update_batch.extend(evm_state_to_hashed_post_state(update));
        }
        MultiProofMessage::EmptyProof { sequence_number, state } => {
            empty_proof_batch.push((sequence_number, state));
        }
        MultiProofMessage::FinishedStateUpdates => {
            *finished = true;
        }
    }

    // Track batch budget using merged targets (accounts + storage slots + wipes).
    prefetch_batch.chunking_length() + state_update_batch.chunking_length()
}
```

### Step 4: Modify Message Loop (lines ~1097-1196)

After receiving a message on `self.rx`, drain additional pending messages **until the merged batch reaches 50 targets** (accounts + storage slots + wipes):

```rust
recv(self.rx) -> message => {
    match message {
        Ok(first_msg) => {
            // Initialize batch collectors
            let mut prefetch_batch: MultiProofTargets = MultiProofTargets::default();
            let mut state_update_batch: HashedPostState = HashedPostState::default();
            let mut empty_proof_batch: Vec<(u64, HashedPostState)> = Vec::new();
            let mut finished = false;
            let mut prefetch_msgs = 0usize;
            let mut state_update_msgs = 0usize;
            let mut batch_target_count: usize = 0;

            // Add first message
            let is_prefetch = matches!(&first_msg, MultiProofMessage::PrefetchProofs(_));
            let is_state_update = matches!(&first_msg, MultiProofMessage::StateUpdate(_, _));
            batch_target_count = Self::add_to_batch(first_msg, &mut prefetch_batch,
                &mut state_update_batch, &mut empty_proof_batch, &mut finished);
            if is_prefetch {
                prefetch_msgs += 1;
            }
            if is_state_update {
                state_update_msgs += 1;
            }

            // Drain additional messages (non-blocking) until merged batch hits 50 targets
            let _enter = debug_span!(
                target: "engine::tree::payload_processor::multiproof",
                "drain control messages"
            ).entered();
            while batch_target_count < MAX_BATCH_TARGETS {
                match self.rx.try_recv() {
                    Ok(next_msg) => {
                        let is_prefetch = matches!(&next_msg, MultiProofMessage::PrefetchProofs(_));
                        let is_state_update = matches!(&next_msg, MultiProofMessage::StateUpdate(_, _));
                        batch_target_count = Self::add_to_batch(next_msg, &mut prefetch_batch,
                            &mut state_update_batch, &mut empty_proof_batch, &mut finished);
                        if is_prefetch { prefetch_msgs += 1; }
                        if is_state_update { state_update_msgs += 1; }
                    }
                    Err(_) => break, // No more messages
                }
            }
            drop(_enter);

            // Process batched messages (see Steps 5-7)
        }
        Err(_) => { /* handle error */ }
    }
}
```

### Step 5: Process EmptyProof Messages First

These have pre-assigned sequence numbers, process individually:

```rust
for (sequence_number, state) in empty_proof_batch {
    proofs_processed += 1;
    if let Some(combined_update) = self.on_proof(
        sequence_number,
        SparseTrieUpdate { state, multiproof: Default::default() },
    ) {
        let _ = self.to_sparse_trie.send(combined_update);
    }
}
```

### Step 6: Process Batched PrefetchProofs

Aggregate all targets into one call:

```rust
if !prefetch_batch.is_empty() {
    if first_update_time.is_none() {
        self.metrics.first_update_wait_time_histogram.record(start.elapsed().as_secs_f64());
        first_update_time = Some(Instant::now());
    }

    self.metrics.batched_prefetch_messages_histogram.record(prefetch_msgs as f64);

    prefetch_proofs_requested += self.on_prefetch_proof(prefetch_batch);
}
```

### Step 7: Process Batched StateUpdates (Key Change)

**Merged HashedPostState is already accumulated in the drain loop:**

```rust
if !state_update_batch.is_empty() {
    if first_update_time.is_none() {
        self.metrics.first_update_wait_time_histogram.record(start.elapsed().as_secs_f64());
        first_update_time = Some(Instant::now());
    }

    self.metrics.batched_state_update_messages_histogram.record(state_update_msgs as f64);
    self.metrics
        .batched_state_update_targets_histogram
        .record(state_update_batch.chunking_length() as f64);

    // Process merged state - now with more targets!
    state_update_proofs_requested += self.on_state_update_hashed(state_update_batch);

    debug!(
        target: "engine::tree::payload_processor::multiproof",
        num_batched = state_update_msgs,
        "Processed batched state updates"
    );
}
```

### Step 8: Add on_state_update_hashed Method

New method that takes pre-hashed state (reuses the same logic as `on_state_update` without re-hashing). It must:
- Update `multi_added_removed_keys`
- Partition into fetched / not-fetched
- Emit `EmptyProof` for fetched targets
- Build `proof_targets` for not-fetched, dispatch with chunking identical to `on_state_update`
- Record existing target/chunk histograms
- Extend `fetched_proof_targets`

```rust
fn on_state_update_hashed(&mut self, hashed_state_update: HashedPostState) -> u64 {
    self.multi_added_removed_keys.update_with_state(&hashed_state_update);

    let (fetched_state_update, not_fetched_state_update) = hashed_state_update
        .partition_by_targets(&self.fetched_proof_targets, &self.multi_added_removed_keys);

    let mut state_updates = 0;

    if !fetched_state_update.is_empty() {
        let _ = self.tx.send(MultiProofMessage::EmptyProof {
            sequence_number: self.proof_sequencer.next_sequence(),
            state: fetched_state_update,
        });
        state_updates += 1;
    }

    // ... same chunking/dispatch/metrics logic as on_state_update ...

    state_updates + chunks
}
```

### Step 9: Handle FinishedStateUpdates Last

```rust
if finished {
    updates_finished = true;
    updates_finished_time = Some(Instant::now());
}

// Check completion after processing all batched messages
if self.is_done(proofs_processed, state_update_proofs_requested,
                prefetch_proofs_requested, updates_finished) {
    break;
}
```

## Why This Works

1. **Fewer dispatches**: N state updates become 1 dispatch with N targets (or chunked appropriately)
2. **Preserved ordering**: Sequence numbers assigned at dispatch time, ProofSequencer handles ordering
3. **More targets per proof**: Aggregated state means more accounts/storage slots per multiproof
4. **Existing deduplication preserved**: `fetched_proof_targets` filtering still works on merged state

## Why This is Safe (Deep Analysis)

### Storage Merge Safety
`HashedStorage::extend()` (line 458) **extends** storage maps, not overwrites:
```rust
self.storage.extend(other.storage.iter().map(|(&k, &v)| (k, v)));
```
All slots from both states are preserved; same slot → later value wins (correct).

### multi_added_removed_keys Safety
`update_with_state()` marks slots based on **final value** only:
- Value = 0 → `insert_removed()`
- Value ≠ 0 → `remove_removed()`

Intermediate removals don't matter - only final state determines `is_removed` status.

### Proof Validity
Proofs show PATHS in the trie. Paths are determined by key hash, which doesn't change.
Whether we update a slot's value in 3 steps or 1 step, the path is identical.

### Target Budget Accuracy
Batch drain stops based on `chunking_length()` (accounts + storage slots + wipes) computed on the merged batches, matching proof sizing and chunking logic used during dispatch and avoiding under-batching on duplicate targets.

## Testing

1. Verify batch size metrics show batching is occurring
2. Verify targets per dispatch increased (target: ~10) and `batched_state_update_targets_histogram` tracks merged targets
3. Verify proof sequencing still correct (results in order)
4. Verify sparse trie updates correctly with batched proofs
5. **Edge cases to test:**
   - Create → Delete → Recreate (same slot)
   - Multiple updates to same account
   - Storage wipe scenarios
   - Account destruction and recreation
6. Unit: `on_state_update` vs `on_state_update_hashed` produce identical dispatches/metrics for the same logical updates
7. Drain loop unit/integration: batching stops at `MAX_BATCH_TARGETS` and records correct message counts and target histogram
8. Batch budget counts deduped targets: repeated accounts/slots in the same drain still allow a full batch and histograms reflect unique targets
9. `FinishedStateUpdates` received alongside queued work still exits once the batch is processed and `is_done` is true
