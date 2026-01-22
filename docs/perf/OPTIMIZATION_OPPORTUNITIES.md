# Reth Hot Path Optimization Opportunities

Optimization opportunities identified through MDBX profiler analysis of reth's parallel state root computation paths.

## MDBX Trace Analysis

**Trace Context**:
- Duration: 48 minutes for 240 blocks
- Total page faults: ~10M (29% major = disk reads)
- Access pattern: **81.5% random, 18.5% sequential** (core pathology)
- File size: 706GB

**Top Bottleneck Tables**:

| Table | Fault % | Major Faults | Top Operation | Time Lost |
|-------|---------|--------------|---------------|-----------|
| StoragesTrie | 20.08% | 497K | GET_BOTH_RANGE | 98ms |
| HashedStorages | 17.71% | 606K | GET_BOTH_RANGE | 134ms |
| HashedAccounts | 14.01% | 528K | SET_RANGE | 101ms |
| PlainAccountState | 9.86% | 361K | DIRECT_GET | 41ms |
| PlainStorageState | 8.91% | 221K | GET_BOTH_RANGE | 47ms |

**Hot Contracts** (by slow access count):
- USDT (`0xdac17f958d2ee523a2206206994597c13d831ec7`): 30K slow ops
- USDC (`0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48`): 12K slow ops  
- WETH (`0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2`): 4.6K slow ops

**Root Cause**: CURSOR_GET operations dominate major faults. The sparse trie hashing/traversal CPU is not the bottleneck. The issues are:
1. **Queue interleaving**: Sorted dispatches get mixed in shared worker queues
2. **Cursor recreation**: Legacy proof path creates new cursors per proof (addressed by #21214)
3. **No worker affinity**: Workers process any key range, causing cursor jumps

---

## Tier 1: High-Impact Architecture Changes

### 1. Shard-Affine Worker Queues (Queue Interleaving Fix)

**Status**: TESTED - Did not help (-5% regression)  
**Impact**: ~~VERY HIGH~~ None  
**Effort**: Medium-Large (1-2 days)

**Benchmark Result**: Implemented 16 sharded channels with work-stealing. Result was -4.99% gas/s regression.
The work-stealing overhead (100µs timeout + checking 15 other shards) outweighed any locality benefit.
Note: #21213 (simple lexicographical sorting before dispatch) was merged and helps - sharding does not.

**Problem**: Storage proof workers share a single unbounded channel (`storage_work_rx`). Even though jobs are dispatched in sorted order, workers pull FIFO from the shared queue. When multiple dispatch batches occur, they interleave:

```
Current Architecture:
                    ┌─────────────────────────────────────┐
                    │   Single unbounded channel          │
                    │   storage_work_tx / storage_work_rx │
                    └─────────────────────────────────────┘
                                    │
        ┌───────────────────────────┼───────────────────────────┐
        ▼                           ▼                           ▼
   Worker 0                    Worker 1                    Worker 2
   (any account)               (any account)               (any account)

Dispatch batch 1: [0x00, 0x01, 0x02, ...]  → enters queue
Dispatch batch 2: [0x10, 0x11, 0x12, ...]  → enters queue (interleaves!)

Worker 0 pulls: 0x00, 0x10, 0x01, 0x11... → cursor jumps between address ranges
```

**Location**: `crates/trie/parallel/src/proof_task.rs:133`
```rust
let (storage_work_tx, storage_work_rx) = unbounded::<StorageWorkerJob>();
// All workers share this single channel
```

**Solution**: Partition jobs by address prefix into 16 sharded channels:

```rust
struct ProofWorkerHandle {
    // Instead of one channel:
    // storage_work_tx: CrossbeamSender<StorageWorkerJob>,
    
    // Use 16 sharded channels (one per first nibble):
    storage_work_tx: [CrossbeamSender<StorageWorkerJob>; 16],
}

fn route_storage_job(&self, hashed_address: B256) -> &CrossbeamSender<StorageWorkerJob> {
    let shard = (hashed_address[0] >> 4) as usize;  // First nibble
    &self.storage_work_tx[shard]
}
```

Each worker would own specific shards, maintaining cursor locality within its address range.

**Expected Impact**: Transform "random walk across entire DB" into "16 mostly-sequential walks". Should significantly reduce the 81.5% random access ratio.

**Trade-offs**: 
- Load imbalance for hot contracts (USDT/USDC concentrated in specific shards)
- Mitigate with work stealing: workers can steal from other shards when idle

**Key Files**: 
- `crates/trie/parallel/src/proof_task.rs` (worker pool creation, job dispatch)
- `crates/engine/tree/src/tree/payload_processor/multiproof.rs` (dispatch_with_chunking)

---

### 2. Account Trie Walk Pipeline Stall

**Status**: ANALYZED - Limited optimization potential  
**Impact**: ~~HIGH~~ Low (blocking is required)  
**Effort**: N/A

**Problem**: The account trie walk blocks waiting for storage proofs:

**Location**: `crates/trie/parallel/src/proof_task.rs:1555-1580`
```rust
while let Some(account_node) = account_node_iter.try_next()... {
    match account_node {
        TrieElement::Leaf(hashed_address, account) => {
            let root = match storage_proof_receivers.remove(&hashed_address) {
                Some(receiver) => {
                    // BLOCKS HERE waiting for storage proof
                    let proof_msg = receiver.recv()?;  // ← Synchronous wait!
```

**Analysis Result**: The blocking is **required** because:
1. `HashBuilder.add_leaf()` requires leaves in sorted order (panics otherwise)
2. Each account needs its storage root before being encoded
3. We cannot skip ahead to process account N+1 while waiting for N

**Solution Option A** (Predictive prefetch ordering): **Already implemented** in #21213 - storage proofs are dispatched in sorted order.

**Solution Option B** (Non-blocking): **Not feasible** - leaves must be added in order.

**Solution Option C** (Batch recv): Marginal benefit, adds complexity.

**Conclusion**: Main optimization (#21213) is already merged. Further improvements would be marginal.

**Key Files**:
- `crates/trie/parallel/src/proof_task.rs` (build_account_multiproof_with_storage_roots)

---

### 3. Cursor Recreation in V2 Proofs

**Status**: Being addressed by #21214  
**Impact**: HIGH  
**Effort**: N/A (leverage existing PR)

**Problem**: Each storage worker creates cursors once at startup:

**Location**: `crates/trie/parallel/src/proof_task.rs:825-830`
```rust
let mut v2_calculator = if self.v2_enabled {
    let trie_cursor = proof_tx.provider.storage_trie_cursor(B256::ZERO)?;
    let hashed_cursor = proof_tx.provider.hashed_storage_cursor(B256::ZERO)?;
    Some(proof_v2::StorageProofCalculator::new_storage(trie_cursor, hashed_cursor))
} else {
    None
};
```

The V2 calculator reuses cursors across proofs within the same worker, which is good. But cursor position is invalidated when switching between accounts, requiring repositioning.

**What #21214 Does**: Keeps cursors alive in `StorageProofCalculator` across proof generations, reducing cursor recreation overhead.

**Remaining Gap**: Even with cursor reuse, if workers process accounts in random order (due to queue interleaving), cursor repositioning is still expensive. This reinforces the need for shard-affine workers (#1 above).

---

### 4. Missed Leaves Sequential Fallback

**Status**: TESTED in PR #19074 - Did not help  
**Impact**: ~~MEDIUM~~ None  
**Effort**: Low-Medium (1-2 hours)

**Problem**: When a storage proof isn't pre-computed, the code falls back to inline computation:

**Location**: `crates/trie/parallel/src/proof_task.rs:1605-1635`
```rust
None => {
    tracker.inc_missed_leaves();
    
    match ctx.cached_storage_roots.entry(hashed_address) {
        dashmap::Entry::Occupied(occ) => *occ.get(),
        dashmap::Entry::Vacant(vac) => {
            // Creates NEW cursors inline during account trie walk
            let root = StorageProof::new_hashed(provider, provider, hashed_address)
                .storage_multiproof(ctx.targets.get(&hashed_address)...)
```

Each missed leaf creates a full proof computation inline, blocking the account trie walk.

**Attempted Solution (PR #19074)**: Pre-dispatch storage proofs for ALL accounts in `storage_prefix_sets`, not just modified accounts in `targets`. This would ensure every account encountered during the trie walk has a pre-computed storage root ready.

**Result**: PR was closed - "Experiment that didnt give good results". The overhead of pre-dispatching all accounts outweighed the benefit of avoiding missed leaves.

**Why batch post-processing won't work**: `HashBuilder.add_leaf()` requires leaves in sorted order. We cannot defer adding an account to post-process its storage root later.

---

## Tier 2: Open PRs to Prioritize

### High Priority

| PR | Title | Impact | Author |
|----|-------|--------|--------|
| #21145 | Clone MDBX RO transactions | Eliminates factory overhead, foundation for other opts | mediocregopher |
| #21214 | V2 Account Proof Computation | Reuses cursors across proofs, addresses #15426/#18032 | mediocregopher |
| #21055 | Zero-Copy DB Serialization | Eliminates heap allocations in writes | DaniPopes |

### Medium Priority

| PR | Title | Impact | Author |
|----|-------|--------|--------|
| #21185 | Bloom Filter for Empty Slots | 10-20% mgas/s on some workloads, 1-2GB memory | gakonst |
| #21140 | Batch Trie Updates Across Blocks | ~50% reduction in write_trie_updates time | gakonst |
| #21202 | Parallelize merge_ancestors | Faster overlay rebuilding | mattsse |
| #21108 | Batch Hashed State Across Blocks | K-way merge, better write locality | gakonst |
| #21089 | Preserve Allocations in wipe() | Reduces allocation churn (limited post-Cancun) | doocho |

### Tracking Issues

- #19511 - Proof Calculation Rewrite
- #17920 - Performance Experiments
- #15426 - Cursor creation overhead in storage multiproofs
- #18032 - Share HashedStorages cursor

---

## Implementation Priority

1. **Foundation**: Land #21145 (MDBX tx clone) to enable other optimizations
2. **Cursor reuse**: Enable #21214 (V2 proofs) by default - addresses cursor recreation
3. ~~**High effort, high reward**: Shard-affine worker routing (1-2d) - addresses queue interleaving~~ **TESTED: Did not help**
4. **Medium effort**: Account trie pipeline optimization - reduces blocking on storage proofs
5. **Validate**: Re-run MDBX profiler to measure improvement in random→sequential ratio

---

## Related Analysis

### Cross-Block Trie Caching (Previously Analyzed)

**Conclusion**: Uncertain ROI

- Each `SparseNode` has `Option<B256>` hash cache that works within a block
- Between blocks, `ClearedSparseStateTrie::from_state_trie()` clears all hashes
- Upper trie levels are almost always invalidated anyway
- Complexity of selective retention, invalidation tracking, fork handling

### Cursor Locality Optimization (PR #20767 - Closed)

**Conclusion**: Did not help

- Attempted to optimize MDBX cursor usage patterns
- Benchmarks showed "high variance and minimal mean improvement" (-60% to +139%)
- Cursor-level optimizations may not yield reliable improvements without key-order scheduling
