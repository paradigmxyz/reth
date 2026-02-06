# Nethermind State / Trie / DB Performance Optimizations - Catalog for Reth

**Date**: 2026-02-06
**Source codebase**: [NethermindEth/nethermind](https://github.com/NethermindEth/nethermind) (C#/.NET)
**Target codebase**: [paradigmxyz/reth](https://github.com/paradigmxyz/reth) (Rust)

**Context**: Nethermind consistently leads execution throughput benchmarks (697 MGas/s on real mainnet blocks per the [GigaGas benchmark](https://www.nethermind.io/blog/getting-ethereum-ready-for-gigagas), Dec 2025). Reth showed 2x performance degradation on merged 100-block payloads. This document catalogs the techniques behind Nethermind's lead.

---

## 1. State Prewarming (Parallel Speculative Execution)

- **Area**: state / caching
- **Technique**: `PreWarmStateOnBlockProcessing`
- **Description**: When a new block arrives, Nethermind speculatively executes all transactions in the block **in parallel** on background threads. This doesn't produce valid state (conflicts are discarded), but populates four concurrent caches (`PreBlockCaches`):
  - `StorageCache`: `ConcurrentDictionary<StorageCell, byte[]>` — pre-reads storage slots
  - `StateCache`: `ConcurrentDictionary<AddressAsKey, Account>` — pre-reads account data
  - `RlpCache`: `ConcurrentDictionary<NodeKey, byte[]?>` — pre-reads trie node RLP from disk
  - `PrecompileCache`: `ConcurrentDictionary<PrecompileCacheKey, Result<byte[]>>` — caches precompile results

  The main sequential block processing then reads from these warm caches instead of hitting disk. Documented as providing "up to 2x speed-up in the main loop block processing."
- **Code location**: `Nethermind.State/PreBlockCaches.cs`, `Nethermind.Trie/PreCachedTrieStore.cs`
- **Evidence of impact**: Official docs: "can lead to an up to 2x speed-up." Enabled by default (`Blocks.PreWarmStateOnBlockProcessing = True`).
- **Applicability to Reth**: Reth already has parallel state root computation but does not pre-warm state *before* sequential EVM execution. The key insight is to use access lists / speculative execution to populate a `ConcurrentHashMap<Address, Account>` and `ConcurrentHashMap<StorageSlot, Value>` before the main execution loop runs. This would hide SSD latency for SLOAD/account reads. Most impactful for validators where block processing time = attestation delay.

---

## 2. HalfPath Key Scheme (Path-Prefixed DB Keys)

- **Area**: DB / trie
- **Technique**: HalfPath node storage
- **Description**: Instead of using only the node's Keccak hash as the database key (32 bytes), Nethermind prefixes keys with the trie path. The key structure is:

  **State nodes** (42 bytes):
  ```
  [section_byte | 8 bytes from path | path_length_byte | 32 byte hash]
  ```

  **Storage nodes** (74 bytes):
  ```
  [section_byte | 32 byte address | 8 bytes from path | path_length_byte | 32 byte hash]
  ```

  Key innovations:
  1. **Sorted by trie locality**: Nodes near each other in the trie are near each other on disk, dramatically improving RocksDB block cache hit rate
  2. **Section separation**: Top-level state nodes (path ≤ 5) get section byte `0`, deeper state gets `1`, storage gets `2`. This isolates hot upper nodes from cold leaves
  3. **Unique keys per path**: Because path is part of the key, different canonical versions of the same node at the same path don't share keys. This enables real-time pruning of old nodes (no longer sharing)
  4. **Readahead-friendly**: Sequential trie traversals benefit from OS/RocksDB readahead since keys are ordered by path

- **Code location**: `Nethermind.Trie/NodeStorage.cs` — `GetHalfPathNodeStoragePathSpan()`
- **Evidence of impact**: [PR #6331](https://github.com/NethermindEth/nethermind/pull/6331) — "speeds up block processing by almost 50%", "database more compressible, shrinking size by about 25%", "database size only grew by 1.28% vs usual 14.62% in 12 days"
- **Applicability to Reth**: Reth's `crates/trie/` uses hash-based keys in MDBX. Implementing path-prefixed keys would:
  - Improve MDBX page cache locality during trie traversal
  - Enable the readahead hint optimization (sequential reads along paths)
  - Reduce database growth by enabling real-time deletion of superseded nodes
  - This is the single highest-impact technique in this catalog

---

## 3. Sharded Dirty Node Cache with Memory Budget

- **Area**: trie / caching
- **Technique**: Sharded in-memory dirty cache with memory-budgeted eviction
- **Description**: Nethermind maintains an in-memory cache of all recently modified ("dirty") trie nodes. Key features:
  - **256 shards** (`_shardedDirtyNodeCount = 256`) to reduce lock contention
  - Each shard is a `ConcurrentDictionary` storing `NodeRecord(TrieNode node, long lastCommit)`
  - Configurable memory budget (default 1 GB, recommended 2-4 GB): `Pruning.CacheMb`
  - Dirty nodes are kept until finalized, then batch-persisted to disk
  - **Commit buffer**: When memory pruning is active, new commits go to a buffer to avoid blocking
  - Memory tracking per-node: `node.GetMemorySize(false) + KeyMemoryUsage`
  - Persisted-node pruning: runs incrementally (`PrunePersistedNodePortion = 0.05`) to avoid spikes

  The cache absorbs 1+ hours of blocks (default `MaxUnpersistedBlockCount = 297`), reducing DB writes by 3x with 2 GB cache.

- **Code location**: `Nethermind.Trie/Pruning/TrieStore.cs`, `Nethermind.Trie/Pruning/TrieStoreDirtyNodesCache.cs`
- **Evidence of impact**: Docs: "reducing total SSD writes by roughly a factor of 3" with 2 GB cache. Metrics: `nethermind_state_skipped_writes` tracks avoided writes.
- **Applicability to Reth**: Reth's trie operations go through `crates/trie/`. A sharded write cache that batches dirty nodes and persists only at finalization boundaries would significantly reduce MDBX write amplification. The 256-shard approach maps well to Rust's `DashMap` or an array of `RwLock<HashMap>`.

---

## 4. Parallel Trie Commit (Concurrent Hashing)

- **Area**: trie
- **Technique**: Multi-level parallel trie commit
- **Description**: During `PatriciaTree.Commit()`, Nethermind decides how many levels of the trie to parallelize based on the number of dirty writes:
  ```
  > 4 * 16 * 16 => parallelize at 3 top levels (up to 4096 tasks)
  > 4 * 16     => parallelize at 2 top levels (up to 256 tasks)
  > 4          => parallelize at 1 top level (up to 16 tasks)
  <= 4         => sequential (no parallelism)
  ```
  Each branch child at the parallelization boundary is committed in a separate `Task.Run()` with `Task.Yield()` for async scheduling. A concurrency quota mechanism prevents over-subscription.
- **Code location**: `Nethermind.Trie/PatriciaTree.cs` — `Commit()`, `CreateTaskForPath()`
- **Evidence of impact**: Key contributor to the 697 MGas/s throughput. The adaptive thresholds avoid parallelism overhead for small updates.
- **Applicability to Reth**: Reth already has parallel state root computation in `crates/trie/trie-parallel/`. However, Nethermind's adaptive threshold approach (based on actual dirty write count rather than fixed configuration) is worth adopting. Additionally, Nethermind parallelizes the *commit* (encoding + hashing), not just the root computation.

---

## 5. Parallel Trie Persistence

- **Area**: trie / DB
- **Technique**: Parallel `PersistNodeStartingFrom` with batched write disposal
- **Description**: When persisting a finalized block's trie changes to disk, Nethermind identifies "parallel start nodes" (subtree roots) and persists each subtree in parallel via `Task.Run()`. Write batches are disposed through a bounded channel with dedicated disposal tasks, preventing write batch disposal from becoming a bottleneck.
- **Code location**: `Nethermind.Trie/Pruning/TrieStore.cs` — `ParallelPersistBlockCommitSet()`
- **Evidence of impact**: `Metrics.SnapshotPersistenceTime` tracks this. Parallelism is critical for large state updates.
- **Applicability to Reth**: Reth writes trie nodes sequentially during state root computation. Parallelizing the DB writes for independent subtrees (using MDBX's multi-cursor support or batched writes) would reduce persistence latency.

---

## 6. DB File Warmer (OS Cache Priming)

- **Area**: DB
- **Technique**: `StateDbEnableFileWarmer`
- **Description**: On startup, Nethermind reads through all SST files of the state database sequentially, populating the OS page cache. Without this, the OS cache can take "several weeks to warm up naturally." This is a simple but highly effective optimization for nodes with 128+ GB RAM.
- **Code location**: `Nethermind.Db.Rocks/` — RocksDB configuration
- **Evidence of impact**: Recommended for 128+ GB systems. Eliminates weeks of cold-start performance degradation.
- **Applicability to Reth**: Trivially implementable. At startup, spawn a background thread that reads all MDBX data files sequentially with `posix_fadvise(SEQUENTIAL)`. This would warm the page cache in minutes instead of weeks.

---

## 7. Adaptive RocksDB Tuning (TuneDbMode)

- **Area**: DB
- **Technique**: Dynamic RocksDB configuration switching
- **Description**: Nethermind dynamically adjusts RocksDB settings based on the current workload phase:
  - **HeavyWrite** (default during sync): Larger write buffers, more memtables, reduced compaction frequency
  - **AggressiveHeavyWrite**: Even more aggressive write buffering for slow SSDs
  - **DisableCompaction**: Completely disables compaction during snap sync, doing one big compaction at the end (~10 min pause)
  - **Normal**: Standard settings for steady-state block processing

  Additional per-memory-tier tuning:
  - 32 GB: Switch from partitioned index to `kBinarySearch` (saves lookup latency, costs ~500 MB RAM)
  - 128 GB: Enable file warmer
  - 350 GB: Disable compression + enable MMAP reads (skip RocksDB block cache entirely)
- **Code location**: `Nethermind.Db.Rocks/`, `Sync.TuneDbMode` configuration
- **Evidence of impact**: DisableCompaction provides "lowest total writes possible" during sync. Compression removal at 350 GB provides "more CPU-efficient encoding."
- **Applicability to Reth**: Reth uses MDBX, not RocksDB, but the principle of workload-adaptive configuration applies. During initial sync, MDBX's `NOSYNC` + larger map sizes could be used, switching to normal settings for steady state.

---

## 8. Struct-Based LRU Cache (Zero-Allocation Cache)

- **Area**: caching
- **Technique**: Array-of-structs LRU replacing LinkedList
- **Description**: Nethermind replaced the standard `Dictionary + LinkedList<LruCacheItem>` LRU cache with an array-of-structs design:
  - `LinkedListNode` + `LruCacheItem` = 48 bytes overhead per entry (64-bit)
  - Array struct with `int prev/next` = 8 bytes overhead per entry
  - Savings: **40 bytes per entry** → **40 MB saved for 1M entries**
  - Also ~20% faster due to reduced pointer chasing (better cache locality)
- **Code location**: `Nethermind.Core/Caching/` — LRU cache implementations
- **Evidence of impact**: [PR #2497](https://github.com/NethermindEth/nethermind/pull/2497) — saves 40MB per million entries, 20% response time improvement
- **Applicability to Reth**: Reth uses various caches (e.g., in `crates/trie/`). Rust's struct layout is already compact, but reviewing cache implementations for unnecessary heap allocations and ensuring struct-of-arrays / arena patterns are used where possible would yield similar gains.

---

## 9. Trie Write Deduplication (Skip Unchanged Writes)

- **Area**: trie
- **Technique**: Net-new write tracking
- **Description**: Nethermind tracks and skips trie node writes that would write unchanged data. Metrics `nethermind_state_skipped_writes` and `nethermind_storage_skipped_writes` count how many writes were avoided. When a trie node's content hasn't changed (same RLP), the write is skipped entirely.
- **Code location**: Trie commit logic in `TrieStore.cs`
- **Evidence of impact**: Visible in Nethermind metrics. For blocks with many SLOADs but few SSTOREs, this avoids significant wasted I/O.
- **Applicability to Reth**: Reth could compare trie node RLP before writing to MDBX. A simple hash comparison or byte comparison at the leaf level would avoid unnecessary DB writes, especially for storage tries where contracts are read but not modified.

---

## 10. Paprika: Custom Page-Based Storage Engine

- **Area**: DB (experimental/future)
- **Technique**: Paprika custom database
- **Description**: A ground-up replacement for RocksDB with Ethereum-specific design:
  1. **4 KB page-based storage** with memory-mapped files (inspired by PostgreSQL/LMDB)
  2. **Copy-on-Write concurrency**: Lock-free readers, single writer
  3. **Path-based access**: Direct account/storage lookup by address, no trie traversal for reads
  4. **Deferred Merkleization**: State root computed as a separate phase after all state changes, not inline
  5. **Finality-aware flushing**: Non-finalized blocks kept in memory (unmanaged memory to avoid GC pressure), flushed to disk only on finality
  6. **SlottedArray pages**: PostgreSQL-style slot arrays for variable-length key-value storage within fixed-size pages
  7. **Combined Keccak+RLP**: Custom implementations that compute Keccak over RLP without intermediate allocations
  8. **Page reuse via abandoned pages**: COW'd pages tracked and recycled after reorg depth threshold

- **Code location**: [NethermindEth/Paprika](https://github.com/NethermindEth/Paprika) — `docs/design.md`
- **Evidence of impact**: Blog: "cut down on the time the EVM spends on data retrieval." Still experimental but represents Nethermind's R&D direction.
- **Applicability to Reth**: Several Paprika ideas are directly applicable:
  - **Deferred Merkleization**: Compute state root only when needed (end of block), not during execution. Reth already does this to some extent.
  - **Finality-aware caching**: Keep N blocks of state diffs in memory, only flush to MDBX at finality (Reth could use this via its `ExExs` or engine pipeline)
  - **Combined Keccak+RLP**: Avoid allocating intermediate RLP buffers — compute Keccak directly from RLP encoding streaming

---

## 11. Path-Based Storage (Flat State Access)

- **Area**: DB / state
- **Technique**: Path-based storage for direct leaf access
- **Description**: Uses the path to the node (i.e., the account address or storage key) as the database key instead of the node hash. Benefits:
  - **O(1) leaf access**: No need to traverse the trie from root to leaf for reads
  - **Natural pruning**: Old values at the same path are overwritten, no pruning needed
  - **Snap serving**: Path-based layout enables efficient range queries for snap sync serving
- **Code location**: [PR #6499](https://github.com/NethermindEth/nethermind/pull/6499) (in development)
- **Evidence of impact**: Blog: "Shorter data access time as reading leaf nodes of MPT does not require traversing the trie."
- **Applicability to Reth**: This is conceptually similar to Reth's existing "flat" account/storage tables (`PlainAccountState`, `PlainStorageState`). Reth already has flat tables for EVM execution. The Nethermind approach confirms that this pattern is correct and could be extended (e.g., using flat tables as the primary state access path, with trie nodes only for state root computation).

---

## 12. CachedTrieStore (Read-Ahead Cache for Traversals)

- **Area**: trie / caching
- **Technique**: Per-traversal trie node cache
- **Description**: For read-only trie traversals (e.g., during proofs or full DB scans), Nethermind wraps the trie store in a `CachedTrieStore` that caches resolved nodes in a `ConcurrentDictionary<(TreePath, Hash256), TrieNode>`. This prevents redundant lookups when multiple operations traverse the same trie regions. For HalfPath scheme, `ReadFlags.HintReadAhead` is used since nodes are ordered.
- **Code location**: `Nethermind.Trie/CachedTrieStore.cs`
- **Evidence of impact**: Essential for operations like `eth_getProof` that traverse multiple storage paths sharing common ancestors.
- **Applicability to Reth**: When computing proofs or doing multi-key lookups, caching resolved trie nodes in a per-operation hash map would avoid redundant MDBX reads.

---

## 13. ReadAhead Hints with Key-Scheme Awareness

- **Area**: DB
- **Technique**: ReadFlag-based readahead separation
- **Description**: When performing full DB scans (pruning, snap serving), Nethermind sets read flags based on the key scheme:
  - **HalfPath**: Sets `HintReadAhead` (nodes are path-ordered, sequential read benefits from prefetching). Further separated into `HintReadAhead2` (deep state) and `HintReadAhead3` (storage) for different RocksDB column families or iterators
  - **Hash-based**: Sets `HintCacheMiss` (nodes are randomly ordered, don't pollute block cache with scan data)
- **Code location**: `Nethermind.Trie/PatriciaTree.cs` — `Accept()` method, `Nethermind.Trie/NodeStorage.cs`
- **Evidence of impact**: Prevents block cache pollution during full pruning, maintains read performance for block processing
- **Applicability to Reth**: MDBX supports `NORDAHEAD` flag and cursor operations. Reth should use `madvise(MADV_SEQUENTIAL)` for trie scans and avoid polluting the page cache during pruning operations.

---

## 14. TrackPastKeys (Incremental Pruning)

- **Area**: trie / pruning
- **Technique**: Real-time obsolete key tracking
- **Description**: With `Pruning.TrackPastKeys = true` (default), Nethermind tracks which trie node keys become obsolete when new nodes replace them at the same path. Combined with HalfPath (where paths uniquely identify nodes), this enables:
  - Real-time deletion of old nodes as new ones are written
  - ~90% pruning effectiveness without full pruning
  - Database growth of only 1.28% over 12 days vs 14.62% without

  Operates via `_persistedHashes` (per-shard `ConcurrentDictionary<HashAndTinyPath, Hash256?>`) that tracks the last persisted hash at each path. When a new hash is persisted at the same path, the old one can be deleted.
- **Code location**: `Nethermind.Trie/Pruning/TrieStore.cs` — `PersistedNodeRecorder()`, `CanDelete()`
- **Evidence of impact**: [PR #6331](https://github.com/NethermindEth/nethermind/pull/6331) — 90% reduction in database growth
- **Applicability to Reth**: Reth's pruning operates via pipeline stages. Implementing per-path tracking of superseded nodes would reduce the need for full trie traversal pruning. This is especially important as Ethereum state grows.

---

## 15. NoResizeClear for ConcurrentDictionary Reuse

- **Area**: caching / memory
- **Technique**: Clear-without-resize pattern
- **Description**: Nethermind's `PreBlockCaches` and `NodeStorageCache` use `NoResizeClear()` — a custom extension that clears the dictionary contents without deallocating the underlying hash table buckets. This avoids repeated allocation/deallocation cycles for per-block caches that are reused every 12 seconds.
- **Code location**: `Nethermind.Core.Collections/CollectionExtensions.cs`, `Nethermind.State/PreBlockCaches.cs`
- **Evidence of impact**: Reduces GC pressure in .NET (analogous to avoiding reallocation in Rust)
- **Applicability to Reth**: Rust equivalent: reuse `HashMap` by calling `.clear()` (which already preserves capacity) or use pre-allocated `Vec`-based maps. Ensure per-block caches in Reth are reused rather than dropped and recreated.

---

## 16. Precompile Result Caching

- **Area**: state / EVM
- **Technique**: Cross-transaction precompile cache
- **Description**: `PreBlockCaches` includes a `PrecompileCache` that caches precompile results (`ConcurrentDictionary<PrecompileCacheKey, Result<byte[]>>`). The key is `(Address, data_hash)`. If the same precompile is called with identical inputs within the same block (common for ecrecover, bn256 pairing), the result is reused.
- **Code location**: `Nethermind.State/PreBlockCaches.cs`
- **Evidence of impact**: [GigaGas benchmark article](https://www.nethermind.io/blog/getting-ethereum-ready-for-gigagas): "precompile caching... eliminated redundant computation" on repetition-heavy blocks
- **Applicability to Reth**: Reth's EVM execution in `crates/evm/` could add a per-block precompile cache. For blocks with many ecrecover calls to the same signature or repeated BN256 pairings, this avoids expensive crypto operations.

---

## 17. Large Array Pool (LargerArrayPool)

- **Area**: memory
- **Technique**: Custom pooling for 1-8 MB buffers
- **Description**: .NET's default `ArrayPool<byte>.Shared` only pools buffers up to 1 MB. Larger allocations are allocated and discarded, causing GC pressure. Nethermind added a `LargerArrayPool` that pools 1-8 MB buffers (sized based on CPU count to correlate with expected parallelism).
- **Code location**: Various pooling utilities
- **Evidence of impact**: [PR #2493](https://github.com/NethermindEth/nethermind/pull/2493) — eliminated GC spikes from large EVM memory allocations
- **Applicability to Reth**: Reth should use arena allocators or `Vec` pools for large temporary buffers in the EVM (e.g., contract memory expansion beyond 1 MB). Rust's allocator story is different from .NET, but the principle applies: pool large buffers rather than allocating/deallocating.

---

## 18. Capped Array Pool for Trie Operations (TrackingCappedArrayPool)

- **Area**: trie / memory
- **Technique**: Size-capped buffer pool for RLP encoding
- **Description**: `TrackingCappedArrayPool` provides pooled byte arrays for trie node RLP encoding/decoding operations. It caps the maximum size to prevent unbounded memory growth and tracks usage for diagnostics.
- **Code location**: `Nethermind.Trie/TrackingCappedArrayPool.cs`
- **Evidence of impact**: Reduces allocation pressure during trie commit operations
- **Applicability to Reth**: Reth's RLP encoding in trie operations should use buffer pools. This is especially important during parallel state root computation where many threads encode trie nodes simultaneously.

---

## Top 5 Most Impactful Techniques (Ranked by Expected Gain for Reth)

### 1. **State Prewarming** (Optimization #1)
**Expected gain**: 30-100% block processing speedup for validator workloads
**Why**: This is Nethermind's single biggest advantage. Reth processes blocks sequentially, hitting SSD for each SLOAD. Prewarming hides all SSD latency by populating caches in parallel before execution. For 12-second slots with ~200 transactions, this is transformative.
**Implementation effort**: Medium-high. Requires running EVM speculatively on background threads, collecting read sets into concurrent caches, and wrapping the state provider to check caches first.

### 2. **HalfPath Key Scheme** (Optimization #2)
**Expected gain**: 30-50% block processing speedup + 25% DB size reduction
**Why**: The most impactful structural change. By ordering DB keys by trie path, every MDBX page cache hit rate improves. This compounds: better cache → fewer I/O → faster blocks → less memory pressure. Additionally enables real-time pruning.
**Implementation effort**: High. Requires changes to `crates/trie/` key encoding and a DB migration path.

### 3. **Sharded Dirty Node Cache** (Optimization #3)
**Expected gain**: 3x reduction in DB writes, smoother block processing
**Why**: Absorbing ~1 hour of trie mutations in memory and batch-persisting at finality boundaries dramatically reduces write amplification. The 256-shard design eliminates lock contention. Combined with memory budget awareness, this prevents OOM while maximizing cache effectiveness.
**Implementation effort**: Medium. Reth could implement this as a layer between trie computation and MDBX writes.

### 4. **DB File Warmer** (Optimization #6)
**Expected gain**: Eliminates cold-start problem (weeks of suboptimal perf → minutes)
**Why**: Trivial to implement, enormous bang-for-buck. A single background thread reading MDBX data files sequentially at startup populates the OS page cache. Essential for any node restart.
**Implementation effort**: Low. ~50 lines of Rust code.

### 5. **Precompile Result Caching** (Optimization #16)
**Expected gain**: Variable, up to 10-50% for ecrecover-heavy / BN256-heavy blocks
**Why**: Many real-world blocks contain repeated precompile calls (e.g., DEX routers calling ecrecover for multiple signatures). Caching is cheap and the payoff per avoided computation is high (ecrecover ~3000 gas, BN256 pairing ~45000+ gas).
**Implementation effort**: Low. A per-block `HashMap<(Address, Hash), Vec<u8>>` in the EVM executor.

---

## Additional References

- [Nethermind: 3 Experimental Approaches to State Database Change](https://www.nethermind.io/blog/nethermind-client-3-experimental-approaches-to-state-database-change) (Jan 2024)
- [Nethermind Performance Tuning Docs](https://docs.nethermind.io/fundamentals/performance-tuning/)
- [Getting Ethereum Ready for GigaGas](https://www.nethermind.io/blog/getting-ethereum-ready-for-gigagas) (Dec 2025)
- [Measuring Ethereum's Execution Limits](https://www.nethermind.io/blog/measuring-ethereums-execution-limits-the-gas-benchmarking-framework) (Nov 2025)
- [Improving Nethermind Performance (LRU Cache, ArrayPool)](https://blog.scooletz.com/2020/11/23/improving-Nethermind-performance) (2020)
- [Paprika Design Document](https://github.com/NethermindEth/Paprika/blob/main/docs/design.md)
- HalfPath PR: [#6331](https://github.com/NethermindEth/nethermind/pull/6331)
- Path-Based Storage PR: [#6499](https://github.com/NethermindEth/nethermind/pull/6499)
