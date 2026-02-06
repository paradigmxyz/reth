# Reth Trie & State Root Performance Analysis

## 1. Architecture Overview

Reth's trie system is split across six crates in `crates/trie/`:

### Crate Map

| Crate | Purpose |
|-------|---------|
| `trie/common/` | Shared types: `Nibbles`, `PrefixSet`, `HashBuilder`, `BranchNodeCompact`, RLP encoding |
| `trie/trie/` | Core sequential algorithms: `StateRoot`, `StorageRoot`, `TrieWalker`, `TrieNodeIter`, proof generation |
| `trie/db/` | MDBX-backed cursor factories (`DatabaseTrieCursorFactory`, `DatabaseHashedCursorFactory`), changeset cache |
| `trie/sparse/` | In-memory sparse trie (`SerialSparseTrie`, `SparseStateTrie`), lazy node revealing, proof-based approach |
| `trie/sparse-parallel/` | `ParallelSparseTrie` — splits trie into upper (depth <2) + 256 lower subtries for parallel hashing |
| `trie/parallel/` | Parallel state root calculator, parallel proof generation, worker pools (`ProofWorkerHandle`) |

### End-to-End Flow: State Root After Block Execution

The modern (preferred) path uses the **sparse trie + proof-based approach**, orchestrated from the engine:

1. **Block execution** proceeds transaction-by-transaction in `crates/engine/tree/src/tree/payload_processor/`.
2. After each transaction (or batch), a `MultiProofMessage::StateUpdate` is sent with the touched accounts/storage.
3. A **multiproof task** (`multiproof.rs`) converts these into proof targets, dispatches to `ProofWorkerHandle` worker pools.
4. Workers compute multiproofs (account + storage) using DB cursors, return `ProofResultMessage`.
5. A **sparse trie task** (`sparse_trie.rs`) receives proofs, reveals nodes into `SparseStateTrie`, applies leaf updates.
6. After all transactions, `root_with_updates()` is called on the sparse trie to compute the final state root.

The **legacy path** (`ParallelStateRoot` in `parallel/src/root.rs`) walks the entire trie using `TrieWalker` + `TrieNodeIter`, pre-computing storage roots in parallel via `tokio::spawn_blocking`. This is explicitly documented as a **fallback** (line 37).

### Two Computation Strategies

| Strategy | File | When Used |
|----------|------|-----------|
| **Sparse Trie (preferred)** | `sparse_trie.rs`, `SparseStateTrie` | Engine pipeline, modern payload processing |
| **Full Trie Walk (fallback)** | `parallel/src/root.rs`, `trie/src/trie.rs` | Initial sync, fallback scenarios |

---

## 2. Parallel Root Analysis

### 2.1 Legacy Parallel Root (`parallel/src/root.rs`)

**Strategy**: Spawn storage root computations as `tokio::spawn_blocking` tasks, then walk the account trie sequentially.

**Bottlenecks**:

1. **Sequential account trie walk** (L136-199): The account trie walker runs on a single thread. Storage roots are parallelized, but the account trie iteration is strictly serial.

2. **Sorted iteration forces ordering** (L103-104): `storage_root_targets.into_iter().sorted_unstable_by_key(...)` sorts all targets before spawning. This is O(n log n) and delays the first spawn.

3. **mpsc::sync_channel(1) per account** (L110): Each storage root gets its own channel. When encountering a leaf, the main thread blocks on `rx.recv()` (L155). If the storage root computation hasn't finished, the account trie walk stalls.

4. **Missed leaves cause synchronous fallback** (L164-176): When a leaf has no pre-computed storage root, it falls back to synchronous `StorageRoot::calculate()` on the main thread, completely blocking progress.

5. **No prefetching**: The tokio runtime's thread pool is used, but there's no explicit prefetching of DB pages or control over thread count beyond `thread_keep_alive(15s)`.

### 2.2 Sparse Trie Approach (`SparseTrieCacheTask`)

**Strategy**: Interleave proof computation with block execution. Uses `crossbeam_channel::select_biased!` to process updates and proof results concurrently.

**Architecture** (from `sparse_trie.rs` L360-418):
```
Execution → MultiProofMessage → SparseTrieCacheTask
                                   ├── on_state_update() → encode leaf updates
                                   ├── process_leaf_updates() → apply to trie (parallel via par_bridge)
                                   ├── dispatch_pending_targets() → send to ProofWorkerHandle
                                   └── on_proof_result() → reveal_decoded_multiproof_v2()
```

**Key parallelism points**:

1. **Worker pools** (`proof_task.rs` L128-277): Pre-spawned storage + account worker pools with dedicated DB transactions. Workers are long-lived, avoiding per-request DB transaction creation overhead.

2. **Storage proof parallelism** (L186-228): Storage workers process proofs independently. Availability tracked via `AtomicUsize` counters.

3. **Parallel storage reveals** (`state.rs` L292-351): When revealing multiproofs, storage trie nodes are revealed in parallel using `par_bridge_buffered()`.

4. **Parallel leaf updates** (`sparse_trie.rs` L585-600): Storage leaf updates applied via `par_bridge_buffered()`.

**Bottlenecks**:

1. **Single-threaded account trie reveal** (`state.rs` L466-499): Account trie nodes are revealed serially — only storage tries get parallel reveals.

2. **Final root computation is serial** (`sparse_trie.rs` L422-426): `root_with_updates()` hashes the entire trie on a single thread. This is the critical path bottleneck.

3. **`select_biased!` ordering** (`sparse_trie.rs` L364): Updates are prioritized over proof results, which can delay applying proofs when execution is fast.

### 2.3 ParallelSparseTrie (`sparse-parallel/src/trie.rs`)

**Strategy**: Split trie into upper subtrie (depth < 2) + 256 lower subtries. Hash lower subtries in parallel with rayon.

**Implementation** (L837-893):
- `update_subtrie_hashes()` takes changed lower subtries, updates their hashes via `into_par_iter().map(...)`.
- Has configurable `ParallelismThresholds` (min_revealed_nodes, min_updated_nodes) to fall back to serial when parallelism overhead exceeds benefit.
- Parallel reveal of nodes: groups by subtrie index, zips with `into_par_iter()` (L268-285).

**Bottleneck**: Upper subtrie hashing is always serial. With depth=2, the upper subtrie contains at most ~256 branch children, so this is typically fast.

---

## 3. Node Access Patterns

### 3.1 DB Read Patterns

**Trie cursors** (`db/src/trie_cursor.rs`): Thin wrappers around MDBX cursors.
- `seek_exact()` and `seek()` are the primary operations — one DB call per trie node lookup.
- Each `TrieWalker::advance()` does one `seek` or `seek_exact` (L286-296 in walker.rs).
- **No batching**: Nodes are fetched one at a time. No read-ahead or batch prefetch.

**Hashed cursors** (`db/src/hashed_cursor.rs`): Direct MDBX cursor operations.
- `seek()` + `next()` pattern for iterating hashed entries.
- **Optimization exists**: `TrieNodeIter` caches `last_seeked_hashed_entry` and `last_next_result` to avoid redundant seeks (L124-157 in node_iter.rs).

### 3.2 Caching

1. **ChangesetCache** (`db/src/changesets.rs`): LRU cache for trie changesets keyed by block number. Used during reorgs. Bounded memory with explicit eviction (L287).

2. **Sparse trie hash memoization**: `SparseNode` stores `hash: Option<B256>` on every node (L1873-1916 in trie.rs). When a node's subtree hasn't changed (not in prefix set), the cached hash is returned immediately — **this is the incremental hashing mechanism**.

3. **Cached storage roots** (`value_encoder.rs`): `DashMap<B256, B256>` shared across workers caches computed storage roots (L69, 227). Avoids recomputing roots for unchanged accounts.

4. **Proof node deduplication** (`state.rs` L471-499): `revealed_account_paths` / `revealed_paths` track which proof paths were already revealed, skipping duplicate work.

5. **No general trie node cache**: There is no LRU/FIFO cache for trie nodes between blocks. Each new state root computation starts with fresh DB cursors.

### 3.3 Prefetching

- **Prewarm/prefetch via multiproofs** (`multiproof.rs` L56-62): `PREFETCH_MAX_BATCH_TARGETS = 512`, `PREFETCH_MAX_BATCH_MESSAGES = 16`. Prefetch proofs are dispatched ahead of execution.
- **No MDBX-level prefetch**: The code does not use `madvise()` or similar to prefetch DB pages.
- **Block Access List (EIP-7928)**: Support exists (`MultiProofMessage::BlockAccessList`) but is `todo!()` at L441.

---

## 4. Bottleneck Shortlist

### B1: Serial Final State Root Computation
- **File**: `crates/engine/tree/src/tree/payload_processor/sparse_trie.rs` L182-183
- **Also**: `crates/trie/sparse/src/state.rs` L886-903
- **Description**: `root_with_updates()` calls `revealed.root()` which runs the full `rlp_node()` loop serially on a single thread.
- **Severity**: **CRITICAL** — This is on the critical path of every block. For blocks with many state changes, this can take 10s-100s of milliseconds.
- **Proposed Fix**: Use `ParallelSparseTrie` which hashes 256 lower subtries in parallel. The infrastructure exists in `sparse-parallel/` but integration into the engine pipeline needs completion.

### B2: Account Trie Reveals Are Serial
- **File**: `crates/trie/sparse/src/state.rs` L466-499
- **Description**: Account multiproof nodes are revealed serially while storage nodes get parallel reveals via `par_bridge_buffered()`.
- **Severity**: **HIGH** — Account trie can have thousands of nodes to reveal per block.
- **Proposed Fix**: Apply the same `par_bridge_buffered()` pattern used for storage reveals (L292-351).

### B3: Per-Node DB Seeks in Walker
- **File**: `crates/trie/trie/src/walker.rs` L286-296
- **Description**: Each `TrieWalker::node()` call does a single `cursor.seek()` or `cursor.seek_exact()`. No batch reads.
- **Severity**: **MEDIUM** — In the legacy path, this dominates I/O time. In the sparse path, it affects proof workers.
- **Proposed Fix**: Implement batch-seek or cursor prefetch. MDBX supports `mdbx_cursor_get` with `MDBX_GET_MULTIPLE`.

### B4: Sorted Iteration Before Spawning Storage Roots
- **File**: `crates/trie/parallel/src/root.rs` L103-104
- **Description**: `sorted_unstable_by_key` sorts all storage root targets before spawning any tasks. Delays first spawn.
- **Severity**: **LOW** (legacy path only) — Could start spawning immediately in hash order.
- **Proposed Fix**: Remove sort or spawn in parallel using `par_iter()` instead of sequential loop.

### B5: HashMap Allocation in Sparse Trie
- **File**: `crates/trie/sparse/src/trie.rs` L337, L346
- **Description**: `SerialSparseTrie` stores nodes in `HashMap<Nibbles, SparseNode>` and values in `HashMap<Nibbles, Vec<u8>>`. Each insert may trigger a rehash. Nibbles keys are small but numerous.
- **Severity**: **MEDIUM** — HashMap overhead is significant with 10K+ nodes per trie.
- **Proposed Fix**: Consider a more cache-friendly data structure (e.g., sorted Vec, BTreeMap, or a custom trie-aware allocator). The `clear()` + reuse pattern already helps (L1014-1023).

### B6: Redundant RLP Encoding
- **File**: `crates/trie/trie/src/trie.rs` L670 (`alloy_rlp::encode_fixed_size`)
- **Also**: `crates/trie/sparse/src/trie.rs` L1584 (leaf RLP), L1606 (extension RLP), L1763 (branch RLP)
- **Description**: RLP encoding happens inline during hash computation. Each `rlp_node()` call clears and refills `rlp_buf`. For nodes not in the prefix set, the cached hash skips encoding — but dirty nodes always re-encode.
- **Severity**: **LOW** — The `rlp_buf` reuse is already good. Could pre-allocate based on expected size.
- **Proposed Fix**: Pool/arena allocation for RLP buffers across parallel subtrie hashing.

### B7: DeferredDrops Accumulate Memory
- **File**: `crates/trie/sparse/src/state.rs` L26-34
- **Description**: `DeferredDrops` collects proof node buffers to avoid expensive deallocations during root computation. But these can accumulate significant memory across many proof reveals.
- **Severity**: **LOW** — Trading memory for latency is intentional.
- **Proposed Fix**: Consider periodic flushing when memory pressure is high.

### B8: Blinded Node Fallback During Leaf Operations
- **File**: `crates/trie/sparse/src/trie.rs` L656, L780-782
- **Description**: When updating or removing a leaf, if a blinded `SparseNode::Hash` is encountered, the operation fails with `BlindedNode` error. This requires an additional proof fetch round-trip.
- **Also**: `reveal_remaining_child_on_leaf_removal` (L1336-1402) does synchronous DB lookups via `provider.trie_node()`.
- **Severity**: **MEDIUM** — Causes extra proof fetch cycles, delaying final root computation.
- **Proposed Fix**: Pre-reveal neighbor nodes when initially revealing proofs (expand proof targets to include siblings).

### B9: BlockAccessList Integration Incomplete
- **File**: `crates/engine/tree/src/tree/payload_processor/sparse_trie.rs` L441
- **Description**: `MultiProofMessage::BlockAccessList(_) => todo!()` — EIP-7928 block access lists could enable perfect prefetching of all required trie nodes before execution begins.
- **Severity**: **HIGH** (opportunity cost) — This is a major optimization opportunity.
- **Proposed Fix**: Implement BAL-based prefetch to pre-reveal all required trie paths before execution starts.

---

## 5. Optimization Candidates (Ranked by Expected Impact)

### Rank 1: Parallel Subtrie Hashing in Engine Pipeline
**Expected impact**: 30-50% reduction in final root computation time  
**Current state**: `ParallelSparseTrie` exists in `sparse-parallel/` with rayon-based parallel hashing of 256 lower subtries. However, the engine pipeline's `SparseTrieCacheTask` uses `SerialSparseTrie`.  
**Implementation**: Replace `SerialSparseTrie` with `ParallelSparseTrie` in `SparseStateTrie<A, S>` type parameters in `SparseTrieCacheTask`.  
**Key files**: `sparse-parallel/src/trie.rs` L837-893, `engine/tree/src/tree/payload_processor/sparse_trie.rs`

### Rank 2: Block Access List (EIP-7928) Prefetching
**Expected impact**: 20-40% reduction in total state root time by eliminating proof fetch round-trips  
**Current state**: Infrastructure exists (`MultiProofMessage::BlockAccessList`) but handler is `todo!()`.  
**Implementation**: Parse BAL at block start, convert to proof targets, dispatch all proofs before execution begins. This eliminates the iterative reveal-execute-reveal cycle.  
**Key files**: `sparse_trie.rs` L441, `multiproof.rs` L3-4

### Rank 3: Parallel Account Trie Reveals
**Expected impact**: 10-20% reduction in proof reveal time  
**Current state**: Storage reveals are parallel (`par_bridge_buffered()`), account reveals are serial.  
**Implementation**: Apply same parallel reveal pattern for account proof nodes. Requires making account trie node reveal lock-free or batching into chunks.  
**Key file**: `sparse/src/state.rs` L466-499

### Rank 4: Incremental Hash Caching Across Blocks
**Expected impact**: 10-30% reduction in per-block root computation for consecutive blocks  
**Current state**: The sparse trie already caches hashes in `SparseNode::hash` fields, and `SparseTrieCacheTask` supports trie reuse across payloads via `into_trie_for_reuse()` with pruning. However, cache effectiveness depends on prune depth.  
**Implementation**: Tune prune depth and max_storage_tries parameters. Consider adaptive pruning based on block content (keep hot paths longer).  
**Key files**: `sparse/src/state.rs` L1162-1175, `sparse_trie.rs` L318-330

### Rank 5: Batch DB Reads for Proof Workers
**Expected impact**: 5-15% reduction in proof computation I/O time  
**Current state**: Each `TrieWalker::node()` call does one MDBX seek. Workers doing storage proofs may seek hundreds of nodes.  
**Implementation**: Implement a prefetching cursor wrapper that reads ahead N nodes when a seek lands on a trie branch with many children. MDBX's `MDBX_GET_MULTIPLE` could be used for dup-sorted tables.  
**Key files**: `db/src/trie_cursor.rs` L65-98, `trie/src/walker.rs` L286-296

### Rank 6: Reduce HashMap Overhead in Sparse Trie
**Expected impact**: 5-10% reduction in allocation-related overhead  
**Current state**: `SerialSparseTrie` uses `HashMap<Nibbles, SparseNode>` which has non-trivial per-entry overhead.  
**Implementation**: Consider a `Vec`-backed structure with path indexing, or a `BTreeMap` for better cache locality. For `ParallelSparseTrie`, the subtries are already smaller so this matters less per-subtrie.  
**Key file**: `sparse/src/trie.rs` L337

### Rank 7: Smarter Proof Target Expansion
**Expected impact**: 5-10% reduction in blinded-node fallback round-trips  
**Current state**: Proofs are fetched exactly for touched paths. Sibling nodes may not be revealed, causing `BlindedNode` errors during leaf operations.  
**Implementation**: When generating proof targets, include sibling paths for accounts with storage changes. This pre-reveals nodes that `update_leaf` and `remove_leaf` might need.  
**Key file**: `sparse/src/trie.rs` L634-768 (update_leaf), L770-1402 (remove_leaf)

### Rank 8: Async Storage Root Computation Pipeline
**Expected impact**: Already partially implemented via `AsyncAccountValueEncoder`  
**Current state**: `value_encoder.rs` implements async storage root computation with pre-dispatched proofs, cached roots, and sync fallback. Stats show `dispatched_count`, `from_cache_count`, `sync_count`.  
**Implementation**: Monitor `sync_count` and `dispatched_missing_root_count` metrics to identify when dispatch coverage is insufficient and tune pre-dispatch logic.  
**Key file**: `parallel/src/value_encoder.rs` L21-37

### Rank 9: Arena Allocation for RLP Buffers
**Expected impact**: 2-5% reduction in allocation overhead during hashing  
**Current state**: Each `rlp_node()` call reuses a single `rlp_buf`, but `branch_value_stack_buf` and `branch_child_buf` are reallocated per branch.  
**Implementation**: Use `RlpNodeBuffers` with pre-sized allocations based on trie depth. The `update_actions_buffers` pool in `ParallelSparseTrie` (L123) already demonstrates this pattern.  
**Key file**: `sparse/src/trie.rs` L1545-1825

---

## 6. Key Observations

### What's Already Well-Optimized

1. **Incremental hashing via prefix sets**: The `PrefixSet` tracks which paths changed, so `rlp_node()` skips unchanged subtrees by checking `hash.filter(|_| !prefix_set_contains(&path))`. This is the most important optimization — it avoids rehashing the entire trie.

2. **Deferred drops**: `DeferredDrops` (L26-34 in state.rs) avoids expensive deallocations during the critical root computation path.

3. **Trie reuse across payloads**: `into_trie_for_reuse()` (L318-330 in sparse_trie.rs) preserves the sparse trie between payloads, with smart pruning based on subtrie heat.

4. **Worker pools with dedicated transactions**: Pre-spawned workers in `ProofWorkerHandle` avoid per-proof transaction creation overhead.

5. **V2 proof format**: The newer `DecodedMultiProofV2` uses `Vec<ProofTrieNode>` instead of `HashMap`, reducing allocation overhead and enabling simpler processing.

6. **Seek deduplication**: `TrieNodeIter` caches the last seeked entry and last next result (L121-172 in node_iter.rs) to avoid redundant DB operations.

### Architecture Gaps vs. Nethermind

1. **Nethermind's half-path optimization**: Nethermind computes state root incrementally during execution, not as a separate step afterward. Reth's architecture separates execution and state root computation, which adds a proof-fetch round-trip cycle.

2. **Nethermind's paprika tree**: Uses a custom page-based trie storage that eliminates DB cursor overhead. Reth uses MDBX with generic cursor abstractions, adding indirection.

3. **Nethermind's parallel witness generation**: Computes witnesses for all touched accounts in parallel before execution. Reth's `MultiProofMessage::PrefetchProofs` is the closest equivalent but is less integrated.
