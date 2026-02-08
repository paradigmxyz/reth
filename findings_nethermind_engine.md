# Nethermind Engine Pipeline Performance Optimizations — Catalog

> Research date: 2026-02-06
> Target: NethermindEth/nethermind (C#/.NET)
> Goal: Identify techniques portable to Reth (`crates/engine/tree/`)

---

## Table of Contents

1. [State Prewarming (Parallel Transaction Pre-Execution)](#1-state-prewarming)
2. [Sender-Grouped Parallel Prewarming](#2-sender-grouped-parallel-prewarming)
3. [Address & Access List Prewarming](#3-address--access-list-prewarming)
4. [Parallel Bloom Calculation](#4-parallel-bloom-calculation)
5. [GC-Aware Block Processing Scheduling](#5-gc-aware-block-processing-scheduling)
6. [Engine API Memory Compaction & Decommit](#6-engine-api-memory-compaction--decommit)
7. [Object Pooling for Transaction Processing Envs](#7-object-pooling-for-transaction-processing-envs)
8. [PreBlockCaches with Clear-on-New-Block](#8-preblockcaches-with-clear-on-new-block)
9. [In-Memory State Pruning with Unpersisted Block Budget](#9-in-memory-state-pruning-with-unpersisted-block-budget)
10. [Dirty Node Sharded Cache](#10-dirty-node-sharded-cache)
11. [DB File Warmer (OS Page Cache Priming)](#11-db-file-warmer)
12. [Adaptive DB Tuning Modes](#12-adaptive-db-tuning-modes)
13. [HalfPath State Database Optimization](#13-halfpath-state-database-optimization)
14. [Array-Based LRU Cache (Allocation-Free)](#14-array-based-lru-cache)
15. [Large Array Pool (Oversized Buffer Reuse)](#15-large-array-pool)
16. [Recovery Queue Bypass for Single Blocks](#16-recovery-queue-bypass)
17. [Main Processing Thread Priority Elevation](#17-main-processing-thread-priority-elevation)
18. [Precompile Result Caching](#18-precompile-result-caching)
19. [Compact Receipt Storage](#19-compact-receipt-storage)
20. [Parallel Block Validation Work](#20-parallel-block-validation-work)

---

## 1. State Prewarming

- **Area**: Block processing / Payload handling
- **Technique**: `PreWarmStateOnBlockProcessing` — parallel speculative transaction execution
- **Description**: Before the main block processing loop begins, Nethermind spawns a background task that speculatively executes all transactions in the incoming block against read-only copies of the world state. This populates the node-storage cache and OS page cache with the trie nodes and storage slots the main processor will need. The main execution then hits warm caches instead of cold SSD reads. Nethermind's docs claim **up to 2x speed-up** on main-loop block processing. The prewarming runs concurrently with — and is cancelled when — the main processing completes.
- **Code location**: `src/Nethermind/Nethermind.Consensus/Processing/BlockCachePreWarmer.cs` — `PreWarmCaches()`, `WarmupTransactions()`
- **Evidence of impact**:
  - Nethermind docs: "up to 2x speed-up in the main loop block processing" ([Performance Tuning](https://docs.nethermind.io/fundamentals/performance-tuning/))
  - Enabled by default (`Blocks.PreWarmStateOnBlockProcessing = true`)
  - Nethermind leads GigaGas benchmarks at 697 MGas/s mean throughput ([Blog](https://www.nethermind.io/blog/getting-ethereum-ready-for-gigagas))
- **Applicability to Reth**: Reth already has `crates/engine/tree/src/tree/payload_processor/prewarm.rs`. The key difference is that Nethermind groups transactions by sender (see #2) and actually runs full `TransactionProcessor.Warmup()` calls — not just address warming. Reth's issue [#17833](https://github.com/paradigmxyz/reth/issues/17833) proposes reusing prewarming results, which Nethermind does implicitly via its pre-block caches.

## 2. Sender-Grouped Parallel Prewarming

- **Area**: Block processing / Prefetching
- **Technique**: Group-by-sender parallelism with sequential intra-sender execution
- **Description**: Transactions are grouped by `SenderAddress`. Different sender groups are warmed in parallel (via `ParallelUnbalancedWork.For`), but transactions within the same sender are executed sequentially. This ensures that balance/nonce/storage changes from tx[N] are visible to tx[N+1] for the same sender, while still maximizing parallelism across independent senders.
- **Code location**: `BlockCachePreWarmer.cs` — `WarmupTransactions()`, `GroupTransactionsBySender()`
- **Evidence of impact**: Without sender grouping, prewarming of same-sender transaction chains would produce incorrect state reads, leading to cache misses on the main thread. This is a correctness optimization that makes prewarming effective rather than just noisy.
- **Applicability to Reth**: Reth's prewarm.rs should consider this sender-grouping pattern. Currently transaction prewarming may not properly handle intra-sender dependencies, reducing cache hit rates on the main execution thread.

## 3. Address & Access List Prewarming

- **Area**: Block processing / Prefetching
- **Technique**: Parallel warming of sender/recipient addresses, withdrawal addresses, and EIP-2930 access lists
- **Description**: Separate from full transaction prewarming, Nethermind runs an `AddressWarmer` as a `ThreadPool` work item that warms up:
  1. All sender and recipient addresses (account trie nodes)
  2. Withdrawal addresses
  3. System transaction access lists (beacon root, blockhash store)
  4. EIP-2930 access lists from each transaction
  
  This runs concurrently with the transaction prewarming via `ThreadPool.UnsafeQueueUserWorkItem`.
- **Code location**: `BlockCachePreWarmer.cs` — `AddressWarmer` class, `WarmupAddresses()`, `WarmupWithdrawals()`
- **Evidence of impact**: Address warming is a lightweight operation that ensures account-level trie nodes are in cache before the heavier transaction execution touches them.
- **Applicability to Reth**: Reth's prewarming could separate address/trie-node warming from full EVM execution warming. Address warming is cheap and has very high hit rates.

## 4. Parallel Bloom Calculation

- **Area**: Block processing
- **Technique**: `ParallelUnbalancedWork.For` for receipt bloom computation
- **Description**: After transactions are processed, Nethermind calculates bloom filters for all receipts in parallel using `ParallelUnbalancedWork.For(0, receipts.Length, ...)`. Each receipt's bloom is computed independently.
- **Code location**: `BlockProcessor.cs` — `CalculateBlooms()`
- **Evidence of impact**: For blocks with hundreds of transactions, this parallelizes a CPU-bound operation that was previously serial.
- **Applicability to Reth**: Reth already computes receipt roots in a background task (`receipt_root_task.rs`). Bloom computation could similarly be parallelized with rayon if not already done.

## 5. GC-Aware Block Processing Scheduling

- **Area**: Engine pipeline
- **Technique**: `GCScheduler` integration with block processing loop
- **Description**: Nethermind's `BlockchainProcessor` integrates with a `GCScheduler` that controls background garbage collection timing:
  - When a block arrives, background GC timer is switched **off** (`SwitchOffBackgroundGC`)
  - After processing completes and queue is empty, background GC timer is switched **on** (`SwitchOnBackgroundGC`)
  
  This prevents GC pauses during active block processing while allowing cleanup during idle periods between blocks.
- **Code location**: `BlockchainProcessor.cs` — `RunProcessingLoop()` calls to `GCScheduler.Instance`
- **Evidence of impact**: .NET GC pauses can be 10-100ms. Avoiding them during the 12-second block slot is critical for attestation timing.
- **Applicability to Reth**: Rust doesn't have GC, but the principle maps to: avoid heavy background work (compaction, flushing, persistence) during active block processing. Reth's persistence state machine could be more careful about when it triggers heavy DB operations.

## 6. Engine API Memory Compaction & Decommit

- **Area**: Engine pipeline / Memory management
- **Technique**: Configurable LOH compaction and memory decommit tied to Engine API call frequency
- **Description**: Nethermind has three config knobs under `Merge.*`:
  - `CollectionsPerDecommit` (default 25): After N engine API calls, request the OS to release process memory
  - `CompactMemory` (default Yes): Compact the Large Object Heap (LOH) after GC
  - `SweepMemory` (implicit Gen2): Controls which GC generation triggers compaction
  
  This addresses .NET's tendency to hold onto freed memory, keeping RSS high.
- **Code location**: `Nethermind.Merge` configuration, engine API handler post-processing
- **Evidence of impact**: Without this, Nethermind nodes can show 40GB+ memory usage due to LOH fragmentation ([Issue #8020](https://github.com/NethermindEth/nethermind/issues/8020)).
- **Applicability to Reth**: Rust doesn't have LOH/GC but the equivalent concern is jemalloc arena fragmentation and mmap retention. Reth could periodically call `jemalloc_ctl::epoch` and `purge` after processing bursts.

## 7. Object Pooling for Transaction Processing Envs

- **Area**: Block processing / Memory management
- **Technique**: `ObjectPool<IReadOnlyTxProcessorSource>` for prewarming environments
- **Description**: Prewarming needs to create multiple isolated world-state views and transaction processors. Rather than allocating these per-block, Nethermind uses `ObjectPool` (`_envPool`) to reuse `IReadOnlyTxProcessorSource` instances. Each parallel warmup thread Gets from the pool, uses it, and Returns it.
- **Code location**: `BlockCachePreWarmer.cs` — `_envPool`, `ReadOnlyTxProcessingEnvPooledObjectPolicy`
- **Evidence of impact**: Avoids heavy per-block allocation of state provider instances. With blocks arriving every 12 seconds, this prevents significant allocation pressure.
- **Applicability to Reth**: Reth could pool `EvmState` or database cursor objects used during prewarming rather than recreating them per block.

## 8. PreBlockCaches with Clear-on-New-Block

- **Area**: Caching / Block processing
- **Technique**: Dedicated pre-block caches populated by prewarming, consumed by main processing, cleared per block
- **Description**: Nethermind maintains `PreBlockCaches` that sit between the prewarming pass and the main execution. Prewarming fills these caches (account state, storage, code, RLP-encoded trie nodes). The main processor reads from them. On a new block, `ClearCaches()` is called immediately. A separate `NodeStorageCache` captures RLP-encoded trie node reads and is also cleared and toggled per block. If caches are non-empty when a new block arrives, a warning is logged.
- **Code location**: `BlockCachePreWarmer.cs` — `PreWarmCaches()` entry point, `ClearCaches()`
- **Evidence of impact**: This is the mechanism that makes prewarming effective — without the cache bridge, prewarming would only help the OS page cache.
- **Applicability to Reth**: Reth's `cached_state.rs` provides similar per-block caching. The key insight is Nethermind's explicit clear-on-new-block + warning-if-stale pattern, which catches bugs in cache lifecycle management.

## 9. In-Memory State Pruning with Unpersisted Block Budget

- **Area**: Caching / Persistence
- **Technique**: `Pruning.MaxUnpersistedBlockCount` — keep up to 297 blocks of state diffs in memory before flushing
- **Description**: Nethermind keeps state diffs for recent blocks in memory (default: 297 blocks ≈ 1 hour of mainnet). State is only persisted to DB when this budget is exceeded. The `MinUnpersistedBlockCount` prevents over-eager flushing. This reduces SSD writes by batching state commits.
- **Code location**: Nethermind.Pruning configuration, state commit logic
- **Evidence of impact**: Combined with `Pruning.CacheMb` (default 1280 MB), this reduces SSD write amplification by ~3x when increased to 2000 MB.
- **Applicability to Reth**: Reth's persistence state machine flushes after configurable intervals. The insight is that keeping MORE state in memory (hundreds of blocks) with explicit budgeting dramatically reduces I/O pressure during block processing.

## 10. Dirty Node Sharded Cache

- **Area**: Caching / Block processing
- **Technique**: `Pruning.DirtyNodeShardBit` — shard the dirty trie node cache into 2^N segments
- **Description**: The dirty node cache (nodes modified but not yet persisted) is sharded by key hash into multiple segments (default 2^8 = 256 shards). This reduces lock contention when multiple threads write to the cache concurrently during block processing and prewarming.
- **Code location**: Nethermind.Pruning configuration (`DirtyNodeShardBit = 8`)
- **Evidence of impact**: Reduces lock contention on the hot path of trie modification during parallel operations.
- **Applicability to Reth**: Reth's in-memory state uses `HashMap`-based structures. Sharding these by key prefix (similar to `DashMap` or striped locks) would reduce contention in multi-threaded scenarios like parallel state root computation.

## 11. DB File Warmer

- **Area**: Caching / Startup
- **Technique**: `Db.StateDbEnableFileWarmer` — sequentially read all DB files at startup to prime OS page cache
- **Description**: On startup, Nethermind optionally reads through all state database files to force them into the OS page cache. Without this, natural cache warming can take **weeks**. This is recommended for machines with 128GB+ RAM where the entire state DB can fit in OS cache.
- **Code location**: Nethermind.Db configuration
- **Evidence of impact**: Eliminates cold-start SSD latency that can persist for weeks after restart.
- **Applicability to Reth**: Reth could implement a startup DB warming pass using `madvise(MADV_WILLNEED)` or sequential reads. This is particularly valuable for MDBX which is memory-mapped. A simple background thread doing sequential reads of the state DB would achieve this.

## 12. Adaptive DB Tuning Modes

- **Area**: Persistence / Block processing
- **Technique**: `Sync.TuneDbMode` — switch RocksDB configuration between sync/processing modes
- **Description**: Nethermind dynamically adjusts RocksDB parameters based on the current operation:
  - `Default`: Balanced for general use
  - `HeavyWrite`: Larger memtables, delayed compaction (used during snap sync)
  - `AggressiveHeavyWrite`: Even larger write buffers
  - `DisableCompaction`: No background compaction (fastest sync, higher memory)
  
  Additional per-mode options include write buffer sizes (`StateDbWriteBufferSize`), compression settings, index types, and MMAP configuration.
- **Code location**: Nethermind.Db and Nethermind.Sync configuration
- **Evidence of impact**: Snap sync from 25 minutes with optimal tuning. Block processing benefits from non-partitioned index (`kBinarySearch`) for lower latency.
- **Applicability to Reth**: Reth uses MDBX which has different tuning knobs, but the principle of mode-switching (sync vs. follow-chain) applies. During tip-following, MDBX could be configured for lower write amplification; during initial sync, for maximum write throughput.

## 13. HalfPath State Database Optimization

- **Area**: Block processing / State access
- **Technique**: Store leaf node data separately from trie structure for O(1) leaf access
- **Description**: Instead of traversing the full Merkle Patricia Trie path for every state read, the HalfPath optimization stores leaf node data (account state, storage values) in a flat key-value mapping alongside the trie. Reads first check the flat mapping (O(1) hash lookup) and only fall back to trie traversal for cache misses. The trie is still maintained for state root computation.
- **Code location**: Nethermind experimental release, state DB layer
- **Evidence of impact**: 
  - **40-50% faster block processing**
  - **80-100% faster state reads**
  - ([Reddit announcement](https://www.reddit.com/r/ethstaker/comments/19an48s/the_nethermind_client_experimental_release_is/))
- **Applicability to Reth**: Reth already uses a similar pattern with separate account/storage tables in MDBX (not pure trie traversal for reads). The equivalent concern is ensuring the hot path for state reads bypasses trie traversal entirely, which Reth's `PlainState` tables already achieve.

## 14. Array-Based LRU Cache

- **Area**: Caching / Memory management
- **Technique**: Replace LinkedList-based LRU with array-of-structs + int indices
- **Description**: Nethermind replaced its LRU cache implementation from `Dictionary<K, LinkedListNode<V>>` + `LinkedList` to `Dictionary<K, int>` + `Node[]` where `Node` is a struct with prev/next as int indices. This eliminates object headers and pointer indirection, saving 40 bytes per entry on 64-bit systems and improving cache locality.
- **Code location**: `LruCache<TKey, TValue>` ([PR #2497](https://github.com/NethermindEth/nethermind/pull/2497))
- **Evidence of impact**: 40MB saved per million cached entries. ~20% faster cache operations due to better CPU cache locality. ([Blog](https://blog.scooletz.com/2020/11/23/improving-Nethermind-performance))
- **Applicability to Reth**: Reth's caches (e.g., in the engine tree's `cached_state.rs`) should use flat-array or `Vec`-backed LRU structures rather than `LinkedHashMap` for better cache line utilization. The `lru` crate in Rust already uses a similar array-based approach.

## 15. Large Array Pool

- **Area**: Memory management
- **Technique**: Custom `LargerArrayPool` for 1-8MB buffers
- **Description**: .NET's default `ArrayPool` discards buffers >1MB, causing allocation spikes during heavy EVM calls. Nethermind added a custom pool layer that reuses buffers in the 1-8MB range.
- **Code location**: `LargerArrayPool` ([PR #2493](https://github.com/NethermindEth/nethermind/pull/2493))
- **Evidence of impact**: Eliminated GC pressure spikes from large EVM memory allocations.
- **Applicability to Reth**: Rust's allocator doesn't have this specific issue, but the principle applies to revm's memory expansion. Reusing `SharedMemory` buffers across transaction executions within a block avoids repeated large allocations.

## 16. Recovery Queue Bypass

- **Area**: Engine pipeline
- **Technique**: Skip recovery queue when processing queue is empty
- **Description**: Nethermind's `BlockchainProcessor` has a two-stage pipeline: recovery queue (sender address recovery/signature verification) → processing queue. When `_queueCount == 1` (only one block, the current one), the block bypasses the recovery channel entirely and goes directly to the processing queue. This eliminates channel overhead and scheduling latency for the common case of tip-following.
- **Code location**: `BlockchainProcessor.cs` — `Enqueue()`: `if (_queueCount > 1) { _recoveryQueue... } else { _blockQueue... }`
- **Evidence of impact**: Reduces latency by one channel hop for the critical single-block-at-tip case.
- **Applicability to Reth**: Reth's engine tree processes `engine_newPayload` synchronously in the handler. The equivalent optimization is ensuring that the hot path for a single new payload doesn't go through unnecessary queuing/channel abstractions.

## 17. Main Processing Thread Priority Elevation

- **Area**: Engine pipeline
- **Technique**: `Thread.CurrentThread.SetHighestPriority()` during block processing
- **Description**: When the processing loop picks up a block, it temporarily elevates the thread priority to highest. This ensures the block processing thread isn't preempted by less critical work (RPC, networking, etc.) during the critical 12-second window.
- **Code location**: `BlockchainProcessor.cs` — `RunProcessingLoop()`: `using var handle = Thread.CurrentThread.SetHighestPriority();`
- **Evidence of impact**: Prevents scheduling jitter from impacting attestation timing.
- **Applicability to Reth**: Reth could use `libc::sched_setscheduler` or `nice` to elevate the block processing thread's priority. This is particularly relevant on busy nodes running RPC alongside consensus.

## 18. Precompile Result Caching

- **Area**: Block processing / EVM
- **Technique**: Cache results of precompile calls within a block
- **Description**: When identical precompile inputs appear repeatedly within a block (common with ModExp, ecrecover), the first result is cached and reused. This was discovered during gas benchmarking when repetitive precompile calls dominated worst-case blocks.
- **Code location**: EVM precompile execution layer (gas benchmarks context)
- **Evidence of impact**: Significant improvement on repetition-heavy blocks. Nethermind's gas benchmarking blog notes this was later excluded from official benchmarks to ensure fairness, but it remains a production optimization.
- **Applicability to Reth**: Reth already has `precompile_cache.rs` in the engine tree. The key is ensuring the cache key includes the full input and that it's cleared per-block. Reth's implementation appears to already follow this pattern.

## 19. Compact Receipt Storage

- **Area**: Persistence
- **Technique**: `Receipt.CompactReceiptStore` and `Receipt.CompactTxIndex`
- **Description**: Nethermind offers compact receipt encoding that trades RPC query performance for reduced database size. The compact format omits redundant fields that can be reconstructed from the block.
- **Code location**: Nethermind.Receipt configuration
- **Evidence of impact**: Significant reduction in receipt DB size, which reduces I/O pressure during block processing when receipts are stored.
- **Applicability to Reth**: Reth already uses compact encoding for receipts. The insight is that receipt storage should be configurable — validators who don't serve RPC can use minimal encoding.

## 20. Parallel Block Validation Work

- **Area**: Block processing
- **Technique**: Parallelize independent validation steps
- **Description**: Nethermind marks the main processing thread via `IsMainProcessingThread` (AsyncLocal) and only performs certain work (like `GetAccountChanges()`) on the main thread. This implies that validation work that doesn't need main-thread state can be offloaded.
- **Code location**: `BlockProcessor.cs` — `if (BlockchainProcessor.IsMainProcessingThread) { block.AccountChanges = ... }`
- **Evidence of impact**: Reduces work on the critical path by deferring non-essential operations.
- **Applicability to Reth**: Reth's engine tree already parallelizes state root computation (`sparse_trie.rs`, `multiproof.rs`). The pattern of explicitly tracking "is this the critical path?" helps identify work that can be deferred.

---

## Top 5 Most Impactful Techniques (Ranked by Expected Performance Gain for Reth)

### 1. 🥇 State Prewarming with Sender-Grouped Parallelism (#1, #2, #3)
**Expected gain: 30-50% block processing latency reduction**

Nethermind's prewarming is their single biggest performance differentiator, delivering up to 2x speedup. The key insights Reth can adopt:
- **Group by sender**: Execute same-sender transactions sequentially, different senders in parallel
- **Reuse results**: Reth issue [#17833](https://github.com/paradigmxyz/reth/issues/17833) already identifies this — track reads during prewarming and feed them into the main execution as a warm cache
- **Separate address warming**: Run a lightweight address/trie-node warming pass concurrently with heavier EVM prewarming
- Maps to: `crates/engine/tree/src/tree/payload_processor/prewarm.rs`

### 2. 🥈 DB File Warming & Adaptive Tuning (#11, #12)
**Expected gain: 20-40% reduction in cold-start and steady-state I/O latency**

Nethermind's file warmer eliminates weeks-long cache warmup periods. For Reth:
- Implement `madvise(MADV_WILLNEED)` scan of MDBX state tables at startup
- Consider mode-switching MDBX parameters between sync and tip-following
- Maps to: MDBX configuration in `crates/storage/`

### 3. 🥉 In-Memory State Budget with Deferred Persistence (#9, #10)
**Expected gain: 15-30% reduction in SSD write pressure during block processing**

Keeping 297 blocks of state in memory before flushing, combined with sharded dirty-node caches:
- Increase the in-memory state budget to absorb write bursts
- Shard concurrent state caches (DashMap or striped-lock patterns)
- Maps to: `crates/engine/tree/src/tree/persistence_state.rs`, `cached_state.rs`

### 4. 🏅 Processing Thread Priority & GC/Background Work Scheduling (#5, #17)
**Expected gain: 5-15% reduction in tail latency / missed attestations**

Ensuring block processing isn't interrupted:
- Elevate thread priority during `engine_newPayload` processing
- Defer heavy persistence/compaction to idle periods between blocks
- Maps to: `crates/engine/tree/src/tree/mod.rs` engine loop

### 5. 🏅 PreBlockCaches Bridge (Prewarming → Main Execution) (#8, #7)
**Expected gain: 10-20% effective cache hit rate improvement**

The explicit cache bridge between prewarming and main execution, with pool-based environment reuse:
- Pool revm `EvmState` / database cursor objects
- Use a dedicated per-block cache that prewarming populates and main execution consumes
- Clear-and-warn pattern catches lifecycle bugs
- Maps to: `crates/engine/tree/src/tree/cached_state.rs`

---

## Sources

| Source | URL |
|--------|-----|
| Nethermind Performance Tuning Docs | https://docs.nethermind.io/fundamentals/performance-tuning/ |
| Nethermind Configuration Reference | https://docs.nethermind.io/fundamentals/configuration/ |
| GigaGas Benchmark Blog | https://www.nethermind.io/blog/getting-ethereum-ready-for-gigagas |
| Gas Benchmarking Framework Blog | https://www.nethermind.io/blog/measuring-ethereums-execution-limits-the-gas-benchmarking-framework |
| BlockProcessor.cs | https://github.com/NethermindEth/nethermind/blob/master/src/Nethermind/Nethermind.Consensus/Processing/BlockProcessor.cs |
| BlockCachePreWarmer.cs | https://github.com/NethermindEth/nethermind/blob/master/src/Nethermind/Nethermind.Consensus/Processing/BlockCachePreWarmer.cs |
| BlockchainProcessor.cs | https://github.com/NethermindEth/nethermind/blob/master/src/Nethermind/Nethermind.Consensus/Processing/BlockchainProcessor.cs |
| Improving Nethermind Performance (Scooletz) | https://blog.scooletz.com/2020/11/23/improving-Nethermind-performance |
| LRU Cache PR #2497 | https://github.com/NethermindEth/nethermind/pull/2497 |
| LargeArrayPool PR #2493 | https://github.com/NethermindEth/nethermind/pull/2493 |
| HalfPath Announcement | https://www.reddit.com/r/ethstaker/comments/19an48s/ |
| Halfpath Blog | https://medium.com/nethermind-eth/nethermind-client-3-experimental-approaches-to-state-database-change-8498e3d89771 |
| Reth Prewarming Reuse Issue | https://github.com/paradigmxyz/reth/issues/17833 |
| Nethermind Memory Config Issue | https://github.com/NethermindEth/nethermind/issues/8433 |
| Execution Payloads Benchmarks Repo | https://github.com/NethermindEth/execution-payloads-benchmarks |
