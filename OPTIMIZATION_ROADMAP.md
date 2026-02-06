# Nethermind → Reth Optimization Roadmap

## Phase 1: Quick Wins (< 1 week each)

| # | Description | Target File | Impact |
|---|-------------|-------------|--------|
| 1 | Cursor metrics: use `as_ref()` instead of `Arc::clone()` per op | `crates/storage/db/src/implementation/mdbx/cursor.rs` | 5-10% cursor overhead reduction |
| 2 | `reinsert_reorged_blocks()`: accept `&[ExecutedBlock]` instead of clone | `crates/engine/tree/src/tree/mod.rs` | Eliminates 2 Vec clones per reorg |
| 3 | Fast-path `find_disk_reorg()`: early-return if persisted tip in canonical chain | `crates/engine/tree/src/tree/mod.rs` | Skips O(n) walk in common case |
| 4 | `BlockBuffer::remove_block()`: replace `VecDeque::retain` with `HashSet` | `crates/engine/tree/src/tree/block_buffer.rs` | O(1) vs O(n) per removal |
| 5 | Wrap overlay in `Arc` in `StateProviderBuilder` | `crates/engine/tree/src/tree/mod.rs` | O(1) clone vs O(n) Vec realloc |
| 6 | Enable MDBX readahead during staged sync, disable for live following | `crates/storage/db/src/implementation/mdbx/mod.rs` | 10-20% faster initial sync I/O |
| 7 | `is_canonical()`: maintain `HashSet<B256>` for O(1) lookup | `crates/engine/tree/src/tree/state.rs` | Eliminates O(n) chain walks |
| 8 | `blocks_by_number`: store `B256` hashes instead of `ExecutedBlock` clones | `crates/engine/tree/src/tree/state.rs` | Less memory, fewer Arc bumps |
| 9 | Parallelize account trie reveals with `par_bridge_buffered()` | `crates/trie/sparse/src/state.rs` L466-499 | 10-20% faster proof reveals |

## Phase 2: Medium Refactors (1-4 weeks each)

| # | Description | Target File | Impact |
|---|-------------|-------------|--------|
| 1 | Batch `write_state_changes` across all blocks (match `write_hashed_state` pattern) | `crates/storage/provider/src/providers/database/provider.rs` L609-637 | 2-5x fewer cursor open/close for batches |
| 2 | Optimize DupSort storage writes: sort by (addr,slot), walk cursor sequentially | `crates/storage/provider/src/providers/database/provider.rs` L2395-2461 | 30-50% fewer MDBX ops |
| 3 | Add hot-state LRU cache at provider layer for top accounts/slots | `crates/storage/provider/src/providers/state/latest.rs` | 20-40% fewer state reads |
| 4 | DB file warmer: background `madvise(MADV_WILLNEED)` scan at startup | `crates/storage/db/src/implementation/mdbx/mod.rs` | Eliminates weeks-long cold start |
| 5 | Cache recently-persisted `ExecutedBlock`s (LRU-64) to avoid trie recomputation | `crates/engine/tree/src/tree/mod.rs` (canonical_block_by_hash) | Critical for reorg performance |
| 6 | Combine save + prune into single MDBX transaction | `crates/engine/tree/src/persistence.rs` L108-126 | Saves 1 fsync per cycle |
| 7 | Sender-grouped parallel prewarming (sequential intra-sender, parallel across) | `crates/engine/tree/src/tree/payload_processor/prewarm.rs` | 30-50% block processing speedup |
| 8 | Deferred history index updates to background task | `crates/storage/provider/src/providers/database/provider.rs` L1228-1276 | 15-25% faster save_blocks |

## Phase 3: Big Bets (1+ month each)

| # | Description | Target File | Impact |
|---|-------------|-------------|--------|
| 1 | Integrate `ParallelSparseTrie` into engine pipeline (replace serial) | `crates/engine/tree/src/tree/payload_processor/sparse_trie.rs` | 30-50% faster state root |
| 2 | EIP-7928 Block Access List prefetching (currently `todo!()`) | `crates/engine/tree/src/tree/payload_processor/sparse_trie.rs` L441 | 20-40% fewer proof round-trips |
| 3 | Adaptive MDBX tuning: switch config between sync and tip-following modes | `crates/storage/db/src/implementation/mdbx/mod.rs` | Lower write amplification |
| 4 | Sharded dirty trie node cache (256 shards, memory-budgeted, batch persist) | `crates/trie/` | 3x reduction in DB writes |
| 5 | Pipeline persistence with double-buffering (overlap I/O with prep) | `crates/engine/tree/src/persistence.rs` | Higher persistence throughput |

## Top 10 Recommendations (Mini-RFCs)

### 1. Sender-Grouped Prewarming
- **Problem:** Prewarming doesn't handle intra-sender tx dependencies, reducing cache hit rate.
- **Nethermind:** Groups txs by sender; sequential within sender, parallel across senders.
- **Reth change:** In `prewarm.rs`, group by `tx.signer()`, use rayon `par_iter` per group. Feed results into `cached_state.rs` pre-block cache with clear-on-new-block semantics.
- **Gain:** 30-50% block processing reduction. **Risk:** Low — prewarming is speculative.

### 2. ParallelSparseTrie for State Root
- **Problem:** Final `root_with_updates()` hashes the entire trie serially on one thread.
- **Nethermind:** Adaptive parallel commit: 1-3 levels of parallelism based on dirty node count.
- **Reth change:** Wire `ParallelSparseTrie` (already in `sparse-parallel/`) into `SparseTrieCacheTask`. Use its 256-subtrie rayon parallelism. Add adaptive thresholds to skip parallelism for small updates.
- **Gain:** 30-50% root computation reduction. **Risk:** Medium — integration complexity.

### 3. Batched Plain State Writes
- **Problem:** `write_state()` called per-block; opens new cursors N times in a batch.
- **Nethermind:** Single commit buffer absorbing hundreds of blocks.
- **Reth change:** In `provider.rs`, merge all `StateChangeset` across the block batch before writing. Use same pattern as `write_hashed_state` (L641-658).
- **Gain:** 2-5x cursor overhead reduction for multi-block batches. **Risk:** Low.

### 4. DB File Warmer
- **Problem:** After restart, OS page cache takes weeks to warm naturally.
- **Nethermind:** `StateDbEnableFileWarmer` reads all DB files sequentially at startup.
- **Reth change:** Spawn background thread at startup that reads MDBX data files with `posix_fadvise(FADV_SEQUENTIAL)`. ~50 lines of code.
- **Gain:** Minutes vs weeks for warm cache. **Risk:** Very low.

### 5. Hot State LRU Cache
- **Problem:** Every SLOAD goes to MDBX B-tree + Compact decode; no app-level cache.
- **Nethermind:** Aggressive caching via `PreBlockCaches` (concurrent dictionaries for accounts, storage, RLP).
- **Reth change:** Add `DashMap`-based LRU in `LatestStateProviderRef` for top ~10K accounts and ~100K slots. Clear on reorg. Invalidate on write.
- **Gain:** 20-40% fewer state reads. **Risk:** Medium — cache invalidation correctness.

### 6. DupSort Write Optimization
- **Problem:** Storage writes do seek-delete-upsert per slot (4000 random ops/block).
- **Nethermind:** Batch-oriented writes with path-sorted keys.
- **Reth change:** In `provider.rs` L2395-2461, sort all entries by (address, slot), open cursor once, walk forward sequentially instead of random seeks.
- **Gain:** 30-50% fewer MDBX ops on storage-heavy blocks. **Risk:** Low.

### 7. Sharded Dirty Node Cache
- **Problem:** Trie writes go directly to MDBX; no write absorption layer.
- **Nethermind:** 256-shard `ConcurrentDictionary` cache with 1-4GB budget, batch-persist at finality.
- **Reth change:** Add a `DashMap`-backed dirty node cache between trie computation and MDBX writes in `crates/trie/`. Persist only at finality boundaries. Track memory usage for eviction.
- **Gain:** 3x reduction in SSD writes. **Risk:** Medium — memory management, crash recovery.

### 8. EIP-7928 Block Access List Prefetching
- **Problem:** `MultiProofMessage::BlockAccessList` handler is `todo!()`.
- **Nethermind:** Pre-warms all addresses + access lists before execution.
- **Reth change:** Parse BAL at block start in `sparse_trie.rs` L441, convert to proof targets, dispatch all proofs before execution. Eliminates iterative reveal-execute-reveal cycle.
- **Gain:** 20-40% state root time reduction. **Risk:** Medium — depends on EIP adoption.

### 9. Thread Priority Elevation
- **Problem:** Block processing thread can be preempted by RPC/networking work.
- **Nethermind:** `Thread.SetHighestPriority()` during block processing.
- **Reth change:** In `crates/engine/tree/src/tree/mod.rs` engine loop, call `libc::setpriority` or `sched_setscheduler` to elevate thread during `on_new_payload`. Restore after.
- **Gain:** 5-15% tail latency reduction. **Risk:** Low — requires CAP_SYS_NICE.

### 10. Trie Write Deduplication
- **Problem:** Unchanged trie nodes may be rewritten to DB unnecessarily.
- **Nethermind:** Tracks `skipped_writes` metric; skips writes when RLP hasn't changed.
- **Reth change:** In trie commit path, compare new node bytes against existing before MDBX `upsert`. Add `reth_trie_skipped_writes` metric.
- **Gain:** Variable, significant for read-heavy blocks. **Risk:** Very low.

## Reth-Only Bugs to Fix

| # | Issue | File | Fix |
|---|-------|------|-----|
| 1 | `canonical_block_by_hash()` recomputes trie updates from DB on every call | `crates/engine/tree/src/tree/mod.rs` L1904-1955 | Cache last N persisted blocks |
| 2 | `MeteredStateHook::on_state()` recomputes counts O(n) per transaction | `crates/engine/tree/src/tree/mod.rs` L232-244 | Track incrementally |
| 3 | `InsertExecutedBlock` path clones block 3 times unnecessarily | `crates/engine/tree/src/tree/mod.rs` L1462-1486 | Use original for last consumer |
| 4 | `get_canonical_blocks_to_persist()` walks backwards from head instead of using BTreeMap range | `crates/engine/tree/src/tree/mod.rs` L1827-1871 | Use `blocks_by_number.range()` |
| 5 | Execution cache miss on non-prewarm path doesn't populate cache for intra-block reuse | `crates/engine/tree/src/tree/cached_state.rs` L310-325 | Populate on miss |
| 6 | `select_biased!` in sparse trie prioritizes updates over proof results, delaying proof application | `crates/engine/tree/src/tree/payload_processor/sparse_trie.rs` L364 | Alternate priority |

## Do NOT Port

| Technique | Why |
|-----------|-----|
| GC-aware scheduling (GCScheduler) | Rust has no GC; irrelevant |
| LOH compaction / memory decommit | .NET-specific memory management |
| Large Array Pool (1-8MB buffers) | Rust allocator doesn't have .NET's >1MB discard behavior |
| Object pooling for TX processing envs | Rust ownership model handles this differently; no GC pressure |
| `NoResizeClear` for ConcurrentDictionary | Rust `HashMap::clear()` already preserves capacity |
| Paprika custom DB engine | Too large a departure; MDBX is well-suited for Reth's architecture |
| HalfPath key scheme | Reth already uses flat `PlainState` tables for reads; trie is only for root computation |
| Compact receipt storage toggle | Reth already uses compact encoding for receipts |
