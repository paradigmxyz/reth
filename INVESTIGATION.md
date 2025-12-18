# Investigation: State Root Mismatch When Batching Multiproof State Updates

**Issue:** [#20223](https://github.com/paradigmxyz/reth/issues/20223)  
**Previous PR (closed):** [#20252](https://github.com/paradigmxyz/reth/pull/20252)  
**Date:** December 18, 2025

## Executive Summary

The state root mismatch when batching consecutive transaction state updates is caused by **batching at the `EvmState` level** instead of the `HashedPostState` level. When `EvmState::extend` merges states, storage slot `is_changed` flags from later transactions overwrite those from earlier transactions, causing changed slots to be incorrectly filtered out.

## Problem Statement

### Goal
Batch consecutive `MultiProofMessage::StateUpdate` messages across different transactions to reduce multiproof computation overhead. Currently, `can_batch_state_update` restricts batching to same-source updates only.

### Symptom
PR #20252 removed the batching restriction and got state root mismatches during testing.

### Discussion Thread Context
- "had the same thought and i think the main issue when doing this is that extend() merges updates and loses the deletion"
- Brian: "the state root only cares about the final state of all slots. If the AddedRemovedKeys is a suspect you can simply not pass it through to the MultiProofTask and see what happens, it's an optional optimization"

## Root Cause Analysis

### The Problematic Code Path

In `process_multiproof_message` (current code in PR #20252):

```rust
// Merge all accumulated updates into a single EvmState payload.
let mut accumulated_iter = ctx.accumulated_state_updates.drain(..);
let (batch_source, mut merged_update) = accumulated_iter.next().expect("...");
for (_, next_update) in accumulated_iter {
    merged_update.extend(next_update);  // <-- PROBLEM HERE
}

// Then convert the merged EvmState to HashedPostState
let hashed_state_update = evm_state_to_hashed_post_state(merged_update);
```

### The Critical Filter in `evm_state_to_hashed_post_state`

```rust
// crates/engine/tree/src/tree/payload_processor/multiproof.rs:224-229
let mut changed_storage_iter = account
    .storage
    .into_iter()
    .filter(|(_slot, value)| value.is_changed())  // <-- THIS FILTER
    .map(|(slot, value)| (keccak256(B256::from(slot)), value.present_value))
    .peekable();
```

### The Bug Scenario

Consider this execution sequence:

```
TX1: slot A changes 0 → 100   (is_changed = true in TX1's EvmState)
TX2: slot A is READ but not written (is_changed = false in TX2's EvmState)
TX3: slot B changes 0 → 200   (is_changed = true in TX3's EvmState)
```

**What happens with EvmState batching:**

1. `merged.extend(tx1_state)` → slot A has `is_changed = true`
2. `merged.extend(tx2_state)` → TX2's entry for slot A **overwrites** TX1's entry
   - Now slot A has `present_value = 100` but `is_changed = false`
3. `evm_state_to_hashed_post_state(merged)` filters by `is_changed()`
4. **Slot A is DROPPED** because `is_changed = false`
5. Sparse trie never sees the 0→100 change for slot A
6. **STATE ROOT MISMATCH**

### Why PR #20252's `record_removals` Didn't Help

The PR tried to fix deletion tracking with a monotonic `record_removals` method. But the actual bug is that **writes are being lost**, not deletions. The `record_removals` approach addressed the wrong problem.

## Why `HashedPostState` Batching Works

Unlike `EvmState`:
- `HashedPostState` has no `is_changed` flags - it only stores final key/value pairs
- `HashedPostState::extend` simply merges values, with later values taking precedence
- `HashedStorage::extend` correctly handles the `wiped` flag for storage wipes

```rust
// crates/trie/common/src/hashed_state.rs:453-459
pub fn extend(&mut self, other: &Self) {
    if other.wiped {
        self.wiped = true;
        self.storage.clear();
    }
    self.storage.extend(other.storage.iter().map(|(&k, &v)| (k, v)));
}
```

## Proposed Solutions

### Design A: Convert Per-TX, Merge HashedPostStates (Recommended)

**Core idea:** Convert each `EvmState` to `HashedPostState` immediately, then merge `HashedPostState`s.

```rust
// In process_multiproof_message for StateUpdate:
let mut merged_hashed_state = HashedPostState::default();

for (source, evm_state) in accumulated_iter {
    // Convert BEFORE merging
    let hashed = evm_state_to_hashed_post_state(evm_state);
    
    // Track added/removed per-update (preserves intermediate deletions)
    self.multi_added_removed_keys.update_with_state(&hashed);
    
    // Merge HashedPostStates (safe - no is_changed semantics)
    merged_hashed_state.extend(hashed);
}

// Use merged HashedPostState for proof computation
self.on_hashed_state_update(batch_source, merged_hashed_state);
```

**Pros:**
- Correct by construction - each TX's changes captured before merge
- `MultiAddedRemovedKeys` updated per-TX preserves deletion history
- Minimal changes to existing architecture
- No new data structures needed

**Cons:**
- Multiple `evm_state_to_hashed_post_state` calls (one per TX instead of one per batch)
- Slightly more memory for intermediate `HashedPostState`s

**Complexity:** Low-Medium

### Design B: Accumulate HashedPostStates Instead of EvmStates

**Core idea:** Change the accumulated buffer type from `Vec<(Source, EvmState)>` to `Vec<(Source, HashedPostState)>`.

```rust
// Change MultiproofBatchCtx
struct MultiproofBatchCtx {
    // Before: accumulated_state_updates: Vec<(Source, EvmState)>
    accumulated_state_updates: Vec<(Source, HashedPostState)>,
    // ...
}

// In process_multiproof_message:
MultiProofMessage::StateUpdate(source, update) => {
    // Convert immediately when receiving
    let hashed = evm_state_to_hashed_post_state(update);
    ctx.accumulated_state_updates.push((source, hashed));
    
    // Later, merge all HashedPostStates
    for (_, hashed_state) in ctx.accumulated_state_updates.drain(..) {
        self.multi_added_removed_keys.update_with_state(&hashed_state);
        merged_hashed_state.extend(hashed_state);
    }
}
```

**Pros:**
- Cleaner separation - conversion happens at message receipt
- Estimates for batching can use `HashedPostState::chunking_length()` directly
- More explicit about what we're actually batching

**Cons:**
- Larger refactor of `MultiproofBatchCtx`
- Need to update `estimate_evm_state_targets` or create equivalent for `HashedPostState`

**Complexity:** Medium

### Design C: Hybrid - Lazy Conversion with Caching

**Core idea:** Keep accumulating `EvmState`s but track which slots were changed in ANY update.

```rust
struct BatchedStateUpdates {
    updates: Vec<(Source, EvmState)>,
    all_changed_slots: HashMap<Address, HashSet<U256>>,  // Track all changed slots
}

impl BatchedStateUpdates {
    fn add(&mut self, source: Source, update: EvmState) {
        // Record which slots are changed BEFORE adding
        for (addr, account) in &update {
            if account.is_touched() {
                let slots = self.all_changed_slots.entry(*addr).or_default();
                for (slot, value) in &account.storage {
                    if value.is_changed() {
                        slots.insert(*slot);
                    }
                }
            }
        }
        self.updates.push((source, update));
    }
    
    fn into_hashed_post_state(self) -> HashedPostState {
        // Merge EvmStates first
        let mut merged = EvmState::default();
        for (_, update) in self.updates {
            merged.extend(update);
        }
        
        // Custom conversion that uses tracked slots instead of is_changed
        evm_state_to_hashed_post_state_with_slots(merged, &self.all_changed_slots)
    }
}
```

**Pros:**
- Single merge at the end (potentially more efficient)
- Preserves all changed slot information

**Cons:**
- More complex data structure
- Need new conversion function
- Duplicates slot tracking logic

**Complexity:** High

### Design D: Disable AddedRemovedKeys for Batched Updates (Quick Validation)

**Core idea:** As Brian suggested, temporarily disable the `MultiAddedRemovedKeys` optimization to validate correctness.

```rust
// For batched updates, pass None for added_removed_keys
let (fetched_state_update, not_fetched_state_update) = hashed_state_update
    .partition_by_targets(&self.fetched_proof_targets, &MultiAddedRemovedKeys::new());  // Empty!
```

**Pros:**
- Fastest to implement
- Good for initial validation that the EvmState merge is the issue

**Cons:**
- Loses optimization - may over-fetch proofs
- Doesn't fix the root cause (EvmState merge issue)
- Only useful for diagnosis, not production

**Complexity:** Very Low

## Recommendation

**Start with Design A** for the following reasons:

1. **Correct by construction** - addresses the root cause directly
2. **Minimal architectural changes** - same buffer types, same flow
3. **Preserves AddedRemovedKeys optimization** - by updating per-TX
4. **Easy to validate** - can compare against Design D (disabled optimization)

### Implementation Steps for Design A

1. Remove `can_batch_state_update` function and its usage
2. In `process_multiproof_message` StateUpdate branch:
   - After accumulating `(Source, EvmState)` tuples, process them differently:
   ```rust
   let mut merged_hashed_state = HashedPostState::default();
   let mut batch_source = None;
   
   for (source, evm_state) in ctx.accumulated_state_updates.drain(..) {
       batch_source = Some(source);
       let hashed = evm_state_to_hashed_post_state(evm_state);
       self.multi_added_removed_keys.update_with_state(&hashed);
       merged_hashed_state.extend(hashed);
   }
   
   batch_metrics.state_update_proofs_requested +=
       self.on_hashed_state_update(batch_source.unwrap(), merged_hashed_state);
   ```

3. Add/update tests for cross-source batching
4. Run existing tests to verify no regressions

## Verification Plan

1. Unit tests for batching scenarios:
   - TX1 writes slot, TX2 reads same slot → slot must appear in merged state
   - TX1 deletes slot, TX2 recreates → correct final value
   - Cross-source batching (PreBlock + Transaction + PostBlock)

2. Integration test:
   - Process a block with known state changes
   - Compare state root with non-batched (single-source) approach

3. Benchmark:
   - Measure proof computation reduction with batching enabled
   - Ensure no performance regression in conversion overhead

## Related Code Locations

| Function | File | Lines |
|----------|------|-------|
| [`evm_state_to_hashed_post_state`](https://github.com/paradigmxyz/reth/blob/453514c48f8457d6b9a27d3d66b9492d704aba32/crates/engine/tree/src/tree/payload_processor/multiproof.rs#L212-L241) | `multiproof.rs` | L212-241 |
| [`process_multiproof_message`](https://github.com/paradigmxyz/reth/blob/453514c48f8457d6b9a27d3d66b9492d704aba32/crates/engine/tree/src/tree/payload_processor/multiproof.rs#L1022-L1189) | `multiproof.rs` | L1022-1189 |
| [`can_batch_state_update`](https://github.com/paradigmxyz/reth/blob/453514c48f8457d6b9a27d3d66b9492d704aba32/crates/engine/tree/src/tree/payload_processor/multiproof.rs#L1591-L1632) | `multiproof.rs` | L1591-1632 |
| [`HashedPostState::extend`](https://github.com/paradigmxyz/reth/blob/453514c48f8457d6b9a27d3d66b9492d704aba32/crates/trie/common/src/hashed_state.rs#L293-L317) | `hashed_state.rs` | L293-317 |
| [`HashedStorage::extend`](https://github.com/paradigmxyz/reth/blob/453514c48f8457d6b9a27d3d66b9492d704aba32/crates/trie/common/src/hashed_state.rs#L453-L459) | `hashed_state.rs` | L453-459 |
| [`MultiAddedRemovedKeys::update_with_state`](https://github.com/paradigmxyz/reth/blob/453514c48f8457d6b9a27d3d66b9492d704aba32/crates/trie/common/src/added_removed_keys.rs#L37-L68) | `added_removed_keys.rs` | L37-68 |
