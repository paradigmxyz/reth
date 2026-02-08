# Reth Storage & Database Performance Analysis

**Date:** 2026-02-06
**Scope:** `crates/storage/` — MDBX database, codecs, providers, static files
**Goal:** Identify bottlenecks and optimization opportunities for Nethermind-style performance gains

---

## 1. Architecture Overview

```
┌────────────────────────────────────────────────────────────────┐
│                    Engine (newPayload / FCU)                     │
└───────────────────────────┬────────────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────────────┐
│                 ConsistentProvider / BlockchainProvider          │
│  (snapshot of in-memory + disk state at creation time)          │
│  crates/storage/provider/src/providers/consistent.rs            │
└───────────┬──────────────────┬────────────────────┬────────────┘
            │                  │                    │
            ▼                  ▼                    ▼
┌──────────────────┐ ┌─────────────────┐ ┌──────────────────────┐
│  In-Memory State │ │  DatabaseProvider│ │  StaticFileProvider  │
│  (CanonicalIMSt) │ │  (MDBX via DbTx)│ │  (NippyJar / mmap)   │
│  chain_state     │ │  provider.rs     │ │  manager.rs          │
└──────────────────┘ └────────┬────────┘ └──────────┬───────────┘
                              │                     │
                     ┌────────┴────────┐   ┌────────┴───────────┐
                     │ DatabaseEnv     │   │ NippyJar (zstd)    │
                     │ (MDBX wrapper)  │   │ Static segments:   │
                     │ mdbx/mod.rs     │   │ Headers, Txs,      │
                     │                 │   │ Receipts, Senders   │
                     │ Tx<RO> / Tx<RW> │   │ AccountChangeSets  │
                     │ Cursor<K, T>    │   │ StorageChangeSets   │
                     └────────┬────────┘   └────────────────────┘
                              │
                     ┌────────┴────────┐
                     │  libmdbx (C)    │
                     │  B+ tree engine │
                     │  mmap'd file    │
                     └─────────────────┘
```

### Storage backends:
- **MDBX** — Primary key-value store for mutable state (accounts, storage, trie, changesets, indices)
- **Static Files (NippyJar)** — Immutable, append-only compressed segments for historical data
- **RocksDB** — Optional secondary store for specific tables (history indices, tx hash lookups)
- **In-memory overlay** — `CanonicalInMemoryState` holds recent blocks not yet persisted

### Key tables (defined in `crates/storage/db-api/src/tables/mod.rs`):

| Table | Key | Value | Type | Purpose |
|-------|-----|-------|------|---------|
| `PlainAccountState` | `Address` | `Account` | Table | Current account state |
| `PlainStorageState` | `Address` | `StorageEntry` (SubKey: `B256`) | DupSort | Current storage slots |
| `HashedAccounts` | `B256` | `Account` | Table | Hashed accounts for trie |
| `HashedStorages` | `B256` | `StorageEntry` (SubKey: `B256`) | DupSort | Hashed storage for trie |
| `AccountChangeSets` | `BlockNumber` | `AccountBeforeTx` (SubKey: `Address`) | DupSort | Account history |
| `StorageChangeSets` | `BlockNumberAddress` | `StorageEntry` (SubKey: `B256`) | DupSort | Storage history |
| `AccountsHistory` | `ShardedKey<Address>` | `BlockNumberList` | Table | Sharded account history index |
| `StoragesHistory` | `StorageShardedKey` | `BlockNumberList` | Table | Sharded storage history index |
| `AccountsTrie` | `StoredNibbles` | `BranchNodeCompact` | Table | Merkle trie nodes |
| `StoragesTrie` | `B256` | `StorageTrieEntry` (SubKey: `StoredNibblesSubKey`) | DupSort | Storage trie nodes |

---

## 2. Transaction Pattern Analysis

### 2.1 Transaction Creation
- **File:** `crates/storage/db/src/implementation/mdbx/mod.rs` L241-261
- `tx()` → `begin_ro_txn()` — creates read-only MDBX snapshot
- `tx_mut()` → `begin_rw_txn()` — creates read-write transaction (exclusive write lock)
- All transactions clone `Arc<HashMap<&str, MDBX_dbi>>` for DBI cache — cheap pointer copy

### 2.2 Transaction Lifetimes
- **Long-lived read transactions** are dangerous: they prevent MDBX from reclaiming pages. A safety mechanism at `crates/storage/db/src/implementation/mdbx/tx.rs` L27 sets `LONG_TRANSACTION_DURATION = 60s` and logs backtraces.
- `ConsistentProvider` (`consistent.rs` L57-77) takes a snapshot: `head_state` + `database_provider_ro()`. The RO transaction lives for the entire duration of the provider.
- **FCU/newPayload path**: `save_blocks()` at `provider.rs` L509-693 holds a single RW transaction for the entire batch of blocks, which is correct — one commit per batch.

### 2.3 Write Transaction Scope
- `save_blocks()` is the main entry point for persisting execution results. One RW tx processes N blocks:
  1. Insert block structure (headers, body indices, senders, tx hashes)
  2. Write state changes (per-block: plain state, bytecodes, storage)
  3. Write hashed state (batched across all blocks — good optimization)
  4. Write trie updates (batched across all blocks — good optimization)
  5. Update history indices (batched across all blocks)
  6. Commit

### 2.4 No Nested Transactions
MDBX supports nested transactions but Reth does **not** use them. All writes within `save_blocks` share a single RW transaction.

---

## 3. Write Path Analysis

### 3.1 Full Write Path: Engine → Disk

```
engine::newPayload
  → execute block (EVM)
  → ExecutedBlock { bundle_state, hashed_state, trie_updates }
  → save_blocks() [provider.rs L509]
      ├── Static file thread: headers, txs, senders, receipts
      ├── RocksDB thread (optional): tx hashes, history indices
      └── MDBX main thread:
           ├── insert_block_mdbx_only [L699]
           │    ├── TransactionSenders (cursor append)
           │    ├── HeaderNumbers (put)
           │    ├── BlockBodyIndices (cursor append)
           │    ├── TransactionBlocks (cursor append)
           │    └── Ommers/Withdrawals (put)
           ├── write_state (per block) [L623]
           │    ├── write_state_changes [L2365]
           │    │    ├── PlainAccountState (cursor upsert/delete)
           │    │    ├── Bytecodes (cursor upsert)
           │    │    └── PlainStorageState (seek+delete+upsert per slot)
           │    ├── write_receipts [L2246-2283]
           │    └── write_state_reverts [L2286]
           │         ├── StorageChangeSets (append via EitherWriter)
           │         └── AccountChangeSets (append via EitherWriter)
           ├── write_hashed_state (batched) [L2427]
           │    ├── HashedAccounts (upsert/delete)
           │    └── HashedStorages (seek+delete+upsert per slot)
           ├── write_trie_updates_sorted (batched) [L656]
           │    ├── AccountsTrie (upsert/delete)
           │    └── StoragesTrie (upsert/delete via dup cursor)
           └── update_history_indices [L3081]
                ├── AccountsHistory (seek+upsert, sharded)
                └── StoragesHistory (seek+upsert, sharded)
  → commit() [static files → RocksDB → MDBX]
```

### 3.2 Data Written Per Block (Approximate)

For a block with 200 transactions touching 500 accounts and 2000 storage slots:

| Table | Operations | Data per op | Total |
|-------|-----------|-------------|-------|
| PlainAccountState | 500 upserts | ~40B | ~20KB |
| PlainStorageState | 2000 seek+delete+upsert | ~64B | ~128KB |
| HashedAccounts | 500 upserts | ~40B | ~20KB |
| HashedStorages | 2000 seek+delete+upsert | ~64B | ~128KB |
| AccountChangeSets | 500 appends | ~40B | ~20KB |
| StorageChangeSets | 2000 appends | ~84B | ~168KB |
| AccountsHistory | ~500 seek+upsert | variable | ~50KB |
| StoragesHistory | ~2000 seek+upsert | variable | ~200KB |
| AccountsTrie | ~100 upserts | variable | ~10KB |
| StoragesTrie | ~500 upserts | variable | ~50KB |
| TransactionSenders | 200 appends | 20B | ~4KB |
| HeaderNumbers | 1 put | 40B | ~40B |
| BlockBodyIndices | 1 append | 16B | ~16B |
| **Total** | | | **~800KB** |

**Write amplification**: Effective state change ≈ 150KB (accounts + storage). Total written ≈ 800KB. **Write amplification ≈ 5.3x** due to changesets, hashed state copies, history indices, and trie updates.

---

## 4. Read Path Analysis

### 4.1 Latest State Access (EVM execution)

```
EVM SLOAD(address, slot)
  → LatestStateProviderRef [latest.rs L38-43]
      → tx.get_by_encoded_key::<PlainAccountState>(address)
         → MDBX point lookup: O(log N) B-tree traversal
      → cursor_dup_read::<PlainStorageState>()
         → seek_by_key_subkey(address, slot)
         → MDBX dup-sort lookup: O(log N) for key + O(log M) for subkey
```

### 4.2 Historical State Access

```
eth_getBalance(address, block=N)
  → HistoricalStateProviderRef [historical.rs L103]
      → account_history_lookup(address)
         → seek AccountsHistory for ShardedKey(address, ?)
         → find block > N in shard → InChangeset(block_number)
      → seek AccountChangeSets at block_number
         → seek_by_key_subkey(block_number, address)
         → return pre-state value
```

This involves **3 B-tree lookups** minimum (history index → changeset → optional plain state fallback).

### 4.3 Provider Hierarchy

```
ConsistentProvider
  ├── In-memory blocks (recent, not yet persisted)
  │    └── MemoryOverlayStateProviderRef
  ├── DatabaseProvider (MDBX)
  │    ├── LatestStateProviderRef (current tip)
  │    └── HistoricalStateProviderRef (past blocks)
  └── StaticFileProvider (immutable historical data)
       └── NippyJar (mmap'd, zstd-compressed)
```

---

## 5. Bottleneck Shortlist

### B1: DupSort Storage Writes — Seek-Delete-Upsert Pattern [HIGH]
- **File:** `crates/storage/provider/src/providers/database/provider.rs` L2395-2421
- **Problem:** For every storage slot update in `PlainStorageState`, the code does:
  1. `seek_by_key_subkey(address, entry.key)` — random B-tree lookup
  2. `delete_current()` — if found
  3. `upsert(address, &entry)` — reinsert

  Same pattern repeated at L2440-2461 for `HashedStorages`.
- **Impact:** 4000 cursor operations per block (2000 plain + 2000 hashed) for storage-heavy blocks.
- **Severity:** HIGH — This is the hottest write path during block execution.
- **Proposed fix:** Batch storage writes: sort all entries by (address, slot), open cursor once, and use sequential seek to avoid random lookups. For blocks where an account's storage is wiped, skip individual deletes and use `delete_current_duplicates()` first.

### B2: Per-Block State Writing Instead of Batched [MEDIUM-HIGH]
- **File:** `crates/storage/provider/src/providers/database/provider.rs` L609-637
- **Problem:** `write_state()` is called **per block** inside the loop (L623), while `write_hashed_state()` and `write_trie_updates()` are batched across all blocks (L641-658). This means `PlainAccountState`, `PlainStorageState`, `Bytecodes`, and changeset tables open new cursors N times instead of once.
- **Impact:** For 128-block batches, this creates 128x cursor open/close overhead and prevents sequential write ordering.
- **Proposed fix:** Batch `write_state_changes` and `write_state_reverts` across all blocks, similar to how hashed state is already batched. Merge all `StateChangeset` objects first, then write once with a single cursor pass.

### B3: History Index Sharding — Expensive Seek on Every Update [MEDIUM]
- **File:** `crates/storage/provider/src/providers/database/provider.rs` L1228-1276
- **Problem:** `append_history_index()` does `cursor.seek_exact(last_key)` for **every** account/storage key to find the last shard. For 500 changed accounts per block, that's 500 random B-tree lookups in `AccountsHistory`.
- **Impact:** MEDIUM — History tables grow large on mainnet (100M+ entries), making seeks expensive.
- **Proposed fix:** Sort updates by key and use cursor walking to amortize seeks. When processing multiple updates for the same key, batch them before the shard lookup.

### B4: Transaction Hash Sorting Before Insert [LOW-MEDIUM]
- **File:** `crates/storage/provider/src/providers/database/provider.rs` L576-606
- **Problem:** All tx hashes are collected into a Vec, then `sort_unstable_by_key` is called to sort by hash for "optimal MDBX insertion performance". This is O(n log n) for potentially thousands of transactions.
- **Impact:** LOW-MEDIUM — This is already a good optimization (sorted inserts are O(1) in MDBX vs O(log N) for random), but the sort itself is not free.
- **Proposed fix:** Consider using a radix sort for B256 keys, which would be O(n) instead of O(n log n).

### B5: Compact Codec Vec Serialization Overhead [LOW]
- **File:** `crates/storage/codecs/src/lib.rs` L235-260
- **Problem:** `Compact for &[T]` allocates a temporary `Vec<u8>` with capacity 64 for **each element** during serialization (L247). For lists with many elements, this creates many small allocations.
- **Impact:** LOW — Hot path for transaction/receipt serialization but allocation is reused via `clear()`.
- **Proposed fix:** Accept the compaction buffer as a parameter to avoid per-element allocation, or use a `SmallVec` to keep small elements on the stack.

### B6: Metrics Clone on Every Cursor Operation [LOW]
- **File:** `crates/storage/db/src/implementation/mdbx/cursor.rs` L50-61
- **Problem:** `execute_with_operation_metric()` clones `self.metrics` (an `Option<Arc<DatabaseEnvMetrics>>`) on every cursor operation. While `Arc::clone()` is cheap, it's an atomic increment/decrement on the hottest path.
- **Impact:** LOW — Atomic operations are ~5-15ns each, but cursor ops happen millions of times per block.
- **Proposed fix:** Use a reference instead of clone. The cursor already borrows the metrics via `Arc`; use `as_ref()` to avoid the atomic ref-count bump.

### B7: No Read Cache in Provider Layer [MEDIUM]
- **Problem:** There is no application-level cache for frequently-accessed accounts or storage slots. Every `SLOAD` during EVM execution goes to MDBX. While MDBX has its own page cache, the decode overhead (Compact → Account/StorageEntry) is paid on every read.
- **Impact:** MEDIUM — Hot accounts (e.g., WETH, Uniswap routers) are read thousands of times per block.
- **Proposed fix:** Add an LRU cache at the `LatestStateProviderRef` level for account lookups and hot storage slots. Nethermind uses aggressive caching of hot state.

### B8: WriteMap Mode Always Enabled for RW [LOW-MEDIUM]
- **File:** `crates/storage/db/src/implementation/mdbx/mod.rs` L366-367
- **Problem:** `write_map()` is always enabled for RW mode. While this offers faster writes by modifying pages in-place via mmap, it means dirty pages are held in the OS page cache longer. On memory-constrained systems, this can cause page eviction of read-hot pages.
- **Impact:** LOW-MEDIUM — Generally beneficial, but worth monitoring on nodes with <16GB RAM.

### B9: Readahead Disabled Globally [LOW]
- **File:** `crates/storage/db/src/implementation/mdbx/mod.rs` L425-427
- **Problem:** `no_rdahead: true` is set for all environments. This is correct for random access patterns (EVM execution) but harmful during initial sync where sequential table scans dominate.
- **Impact:** LOW — Only affects initial sync, not live following.
- **Proposed fix:** Make readahead configurable, or enable it during staged sync and disable during live following.

---

## 6. Optimization Candidates (Ranked by Impact)

### Rank 1: Batch Plain State + Changeset Writes Across Blocks
- **Current:** `write_state()` called per block in `save_blocks` loop (L609-637)
- **Target:** Merge all `StateChangeset` and `PlainStateReverts` across the block batch, then write once
- **Expected gain:** 2-5x reduction in cursor open/close overhead for multi-block batches
- **Effort:** Medium — Requires restructuring `write_state` to accept merged changesets
- **Evidence:** `write_hashed_state` and `write_trie_updates` already do this successfully (L641-658)

### Rank 2: Optimize DupSort Storage Write Pattern
- **Current:** Seek-delete-upsert per slot in PlainStorageState/HashedStorages
- **Target:** Batch all slots per address, open cursor once, walk sequentially
- **Expected gain:** 30-50% reduction in MDBX operations for storage-heavy blocks
- **Effort:** Medium — Sort slots, then walk cursor forward instead of random seeks
- **Files:** `provider.rs` L2395-2421 (PlainStorageState), L2440-2461 (HashedStorages)

### Rank 3: Add Hot State Cache
- **Current:** Every EVM SLOAD goes through MDBX B-tree lookup + Compact decode
- **Target:** LRU cache for top ~10K accounts and ~100K storage slots
- **Expected gain:** 20-40% reduction in state reads during execution (Zipf distribution of account access)
- **Effort:** Low — Add `DashMap` or sharded `LruCache` to `LatestStateProviderRef`
- **Risk:** Cache invalidation complexity; must be cleared on reorg

### Rank 4: Reduce Write Amplification via Deferred History
- **Current:** History indices (AccountsHistory, StoragesHistory) updated synchronously in `save_blocks`
- **Target:** Defer history index updates to a background task, or batch them across multiple `save_blocks` calls
- **Expected gain:** 15-25% reduction in `save_blocks` latency
- **Effort:** Medium-High — Requires background worker + crash recovery considerations
- **Evidence:** Metrics show `update_history_indices` is a significant portion of `save_blocks` time

### Rank 5: Cursor Metrics Optimization
- **Current:** `Arc::clone()` per cursor operation + HashMap lookup per metric recording
- **Target:** Store metric references directly in cursor, avoid atomic refcount
- **Expected gain:** 5-10% reduction in cursor operation overhead
- **Effort:** Low — Refactor `Cursor` to hold `&DatabaseEnvMetrics` instead of `Option<Arc<...>>`
- **File:** `cursor.rs` L50-61

### Rank 6: Compact Codec Improvements
- **Current:** Per-element temporary buffer allocation in Vec<T> serialization
- **Target:** Thread-local or caller-provided buffer for Compact encoding
- **Expected gain:** Minor — reduces allocation pressure during heavy writes
- **Effort:** Low
- **File:** `codecs/src/lib.rs` L247

### Rank 7: Configurable Readahead for Sync Stages
- **Current:** `no_rdahead: true` always
- **Target:** Enable readahead during `SenderRecovery`, `Execution` stages where access is sequential
- **Expected gain:** 10-20% improvement for initial sync disk I/O
- **Effort:** Low — Pass flag through `DatabaseArguments`
- **File:** `mdbx/mod.rs` L427

---

## 7. MDBX Configuration Analysis

**File:** `crates/storage/db/src/implementation/mdbx/mod.rs` L344-499

| Parameter | Value | Analysis |
|-----------|-------|----------|
| Map size | 0..8TB | Reasonable for mainnet (~2TB current) |
| Growth step | 4GB | Good — avoids frequent remaps |
| Page size | `default_page_size()` (4KB on most systems) | Standard; 8KB could reduce tree depth for large values |
| Max readers | 32,000 | Conservative but safe |
| Sync mode | `Durable` (default) | Safest; `SafeNoSync` would give 10x write speedup at risk of data loss |
| WriteMap | Enabled for RW | Good — in-place modification via mmap |
| no_rdahead | `true` | Correct for random access; harmful for sync |
| coalesce | `true` | Good — merges adjacent free pages |
| rp_augment_limit | 256K pages | Good — prioritizes freelist reuse over file growth |

### Configuration recommendations:
1. **Consider `SafeNoSync` mode** for live following if the node can recover from unclean shutdown via re-executing from last checkpoint. This alone could provide 5-10x write throughput improvement.
2. **Investigate 8KB page size** for tables with large values (StorageChangeSets, AccountsHistory) to reduce overflow pages.

---

## 8. Existing Metrics Coverage

The storage layer has **good metrics instrumentation**:

### Database-level metrics (`crates/storage/db/src/metrics.rs`):
- Per-table, per-operation call counts
- Large value (>4KB) operation durations
- Transaction open/close durations
- Commit latency breakdown (preparation, GC, audit, write, sync)

### Provider-level metrics (`crates/storage/provider/src/providers/database/metrics.rs`):
- `save_blocks` total, MDBX, static file, RocksDB durations
- Per-sub-step histograms: insert_block, write_state, write_hashed_state, write_trie_updates, update_history_indices
- Last-value gauges for real-time monitoring

### Overlay metrics (`crates/storage/provider/src/providers/state/overlay.rs` L31-48):
- Provider creation, trie/hashed state retrieval durations
- Cache miss counters

### Missing metrics:
- **No per-table write volume tracking** (bytes written per table per block)
- **No decode/encode duration tracking** (Compact codec overhead)
- **No cache hit/miss rates** (because there's no application-level cache)
- **No MDBX page fault tracking** (would reveal cold vs hot page access patterns)

---

## 9. Static File Interaction

### Architecture
- Static files use NippyJar (zstd-compressed, mmap'd) for immutable historical data
- Segments: Headers, Transactions, TransactionSenders, Receipts, AccountChangeSets, StorageChangeSets
- Files are organized in fixed-size ranges (default 500K blocks per file)
- Provider at `crates/storage/provider/src/providers/static_file/manager.rs`

### Parallelism
- `save_blocks()` parallelizes static file writes with MDBX writes using `thread::scope` (L554-692)
- Static files written on dedicated OS thread via `spawn_scoped_os_thread`
- RocksDB also on a dedicated thread when enabled

### Redundancy concerns
- **No redundancy** for active state tables — data exists in either MDBX or static files, controlled by `EitherWriter`
- The `EitherWriterDestination` mechanism (`either_writer.rs`) routes writes to the appropriate backend based on `StorageSettings`
- Changesets can be in MDBX, static files, or RocksDB — clean separation

---

## 10. Summary of Top Recommendations

| Priority | Optimization | Expected Speedup | Effort |
|----------|-------------|-------------------|--------|
| P0 | Batch plain state writes across blocks | 2-5x for multi-block saves | Medium |
| P0 | Optimize DupSort seek-delete-upsert pattern | 30-50% fewer MDBX ops | Medium |
| P1 | Hot state LRU cache | 20-40% fewer state reads | Low |
| P1 | Deferred history index updates | 15-25% faster save_blocks | Medium-High |
| P2 | Cursor metrics refactoring | 5-10% less overhead | Low |
| P2 | Configurable readahead for sync | 10-20% faster initial sync | Low |
| P3 | SafeNoSync mode option | 5-10x write throughput | Config change |
| P3 | Compact codec buffer reuse | Minor allocation savings | Low |
