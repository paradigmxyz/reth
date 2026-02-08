# Hot State LRU Cache Exploration

## 1. Current State Read Path

### EVM SLOAD → MDBX Read Trace

```
EVM SLOAD instruction
  → revm State<DB>::storage()
    → revm checks CacheAccount in-memory (per-block cache)
    → on miss: StateProviderDatabase<DB>::storage_ref()
      → StateProvider::storage(address, storage_key)
        → LatestStateProviderRef::storage()  [latest.rs:155-167]
          → tx.cursor_dup_read::<PlainStorageState>()
            → seek_by_key_subkey(account, storage_key)
              → MDBX B-tree lookup + Compact decode
```

### Key files in the path:
- **StateProvider trait**: [`crates/storage/storage-api/src/state.rs#L34-L48`](file:///home/ubuntu/reth/crates/storage/storage-api/src/state.rs#L34-L48) — defines `fn storage(&self, account: Address, storage_key: StorageKey) -> ProviderResult<Option<StorageValue>>`
- **LatestStateProviderRef**: [`crates/storage/provider/src/providers/state/latest.rs#L155-L167`](file:///home/ubuntu/reth/crates/storage/provider/src/providers/state/latest.rs#L155-L167) — opens a dup cursor on `PlainStorageState` table, seeks by key+subkey
- **StateProviderDatabase**: [`crates/revm/src/database.rs#L105-L171`](file:///home/ubuntu/reth/crates/revm/src/database.rs#L105-L171) — bridges revm `Database` trait to reth `EvmStateProvider`
- **BasicBlockExecutor**: [`crates/evm/evm/src/execute.rs#L528-L541`](file:///home/ubuntu/reth/crates/evm/evm/src/execute.rs#L528-L541) — wraps DB in `State::builder().with_database(db).with_bundle_update().without_state_clear().build()`

### Account reads follow a similar path:
```
EVM basic() → StateProviderDatabase::basic_ref()
  → AccountReader::basic_account(address)
    → LatestStateProviderRef::basic_account()  [latest.rs:40-42]
      → tx.get_by_encoded_key::<PlainAccountState>(address)
        → MDBX point lookup + Compact decode
```

## 2. Existing Caching Mechanisms

### 2a. Revm's Per-Block `State<DB>` Cache (intra-block)

The `BasicBlockExecutor` creates `State::builder().with_database(db).with_bundle_update().without_state_clear().build()`. Revm's `State` maintains a `CacheAccount` map internally — once an account/slot is read within a block execution, subsequent reads within the same block are served from memory. However, this cache is **per-block** and does not persist across blocks.

### 2b. `ExecutionCache` / `CachedStateProvider` (cross-block, engine tree)

**This is the primary existing cross-block cache.** Located in [`crates/engine/tree/src/tree/cached_state.rs`](file:///home/ubuntu/reth/crates/engine/tree/src/tree/cached_state.rs).

Architecture:
- Uses `fixed-cache` crate (v0.1.7) — a concurrent, fixed-size hash map with O(1) epoch-based invalidation
- Three separate caches: accounts (`Address → Option<Account>`), storage (`(Address, StorageKey) → StorageValue`), bytecode (`B256 → Option<Bytecode>`)
- Default allocation: 88.88% storage, 5.56% accounts, 5.56% bytecode
- `CachedStateProvider` wraps any `StateProvider` and intercepts reads
- Cross-block persistence via `SavedCache` — tracks block hash and uses `Arc<()>` reference counting to ensure exclusive access
- `PayloadExecutionCache` (in `payload_processor/mod.rs`) stores a `SavedCache` behind `Arc<RwLock<Option<SavedCache>>>` and matches on `parent_hash` to reuse across sequential blocks
- After block execution, `insert_state(&BundleState)` updates the cache with all modified accounts/storage/code from the block
- On reorg (parent hash mismatch): clears and reuses the cache structure

**Invalidation strategy:**
- After each block: `insert_state()` overwrites entries for all modified accounts/slots
- On `SELFDESTRUCT` of a contract: clears entire account + storage caches (rare post-Dencun)
- On fork/reorg: `SavedCache` is cleared if parent hash doesn't match
- Epoch-based bulk invalidation via `fixed-cache`'s `EPOCHS` feature

### 2c. Overlay State Provider (trie computation)

[`crates/storage/provider/src/providers/state/overlay.rs`](file:///home/ubuntu/reth/crates/storage/provider/src/providers/state/overlay.rs) — uses `DashMap<BlockNumber, Overlay>` to cache trie updates and hashed post-state per block. This is for trie/proof computation, NOT for EVM state reads.

### 2d. Other caches in the codebase

- **RPC layer**: `schnellru::LruMap` for blocks/receipts/headers in [`crates/rpc/rpc-eth-types/src/cache/`](file:///home/ubuntu/reth/crates/rpc/rpc-eth-types/src/cache)
- **Networking**: `schnellru` for peer discovery, DNS, invalid headers
- **Precompile cache**: `DashMap` in [`crates/engine/tree/src/tree/precompile_cache.rs`](file:///home/ubuntu/reth/crates/engine/tree/src/tree/precompile_cache.rs)
- **RPC bytecode**: `DashMap` code_store in `crates/storage/rpc-provider/`

## 3. Where Should a New Cache Live?

### Current situation: The cache ALREADY EXISTS

The `CachedStateProvider` + `ExecutionCache` system in `crates/engine/tree/src/tree/cached_state.rs` is already a hot state LRU cache for the engine's block execution path. It:
- Caches accounts, storage, and bytecode
- Persists across blocks via `SavedCache`
- Uses `fixed-cache` (open-addressing hash map, power-of-two sized, epoch-tagged)
- Is updated after each block with `BundleState` diffs
- Is cleared on reorg

### What's NOT cached

1. **RPC reads**: When an `eth_call` or `eth_getStorageAt` hits the latest state provider, it goes directly through `LatestStateProviderRef` → MDBX. The `CachedStateProvider` is only used in the engine tree's block execution path, not for RPC reads.

2. **Historical state reads**: `HistoricalStateProviderRef` always hits the DB + changesets.

3. **Staged sync reads**: During initial sync, the pipeline stages read directly from MDBX.

### Potential layers for an additional/shared cache

| Layer | Pros | Cons |
|-------|------|------|
| **Wrap `LatestStateProviderRef`** | Simple, catches all latest-state reads including RPC | Only helps latest state, not historical; must invalidate on every block commit |
| **Wrap `DatabaseProvider`** | Catches everything including RPC, historical state | Complex invalidation; historical reads are less cacheable |
| **Inside `ConsistentDbView`** | Natural boundary for trie computation | Too narrow scope; only used for proof generation |
| **Shared `ExecutionCache` exposed to RPC** | Reuses existing infrastructure; engine already populates it | Thread safety concerns; engine execution shouldn't be slowed by RPC reads; cache is currently tied to payload processor lifecycle |

### Recommendation

The highest-impact approach would be to **share the existing `ExecutionCache` with RPC providers**. The engine tree already populates the cache with every account/storage/code touched during block execution. If RPC `eth_call` and `eth_getStorageAt` could read from this same cache, many hot-state reads would be served from memory.

The second approach would be a **new cache layer wrapping `LatestStateProviderRef`** specifically for RPC, populated on read-through (miss → MDBX → insert).

## 4. Implementation Options

### Available libraries (already in Cargo.toml)

| Library | Version | Current usage |
|---------|---------|---------------|
| `schnellru` | 0.2 | Networking, RPC block/receipt cache, invalid headers |
| `dashmap` | 6.0 | Overlay cache, precompile cache, static file jars |
| `fixed-cache` | 0.1.7 | `ExecutionCache` (accounts, storage, bytecode) |

### Cache data structures comparison

| Structure | Thread-safe | Eviction | Best for |
|-----------|-------------|----------|----------|
| `fixed-cache` | Yes (lock-free) | Open-addressing collision | High-throughput concurrent reads/writes (engine) |
| `schnellru::LruMap` | No (needs external lock) | True LRU | Single-threaded or behind RwLock |
| `DashMap` | Yes (sharded) | None (manual) | Concurrent maps without size bounds |
| `parking_lot::RwLock<schnellru::LruMap>` | Yes | True LRU | Moderate-throughput shared cache |

### Cache key design

```rust
// Accounts
Key: Address (20 bytes)
Value: Option<Account> (nonce: u64, balance: U256, bytecode_hash: Option<B256>) ≈ 72 bytes

// Storage  
Key: (Address, StorageKey) (20 + 32 = 52 bytes)
Value: StorageValue (U256, 32 bytes)

// Entry sizes (fixed-cache, 128-byte aligned):
// Account entry: 128 bytes
// Storage entry: 128 bytes
```

### Sizing estimates

| Cache | Entries | Memory |
|-------|---------|--------|
| Accounts | 10K | ~1.3 MB |
| Storage slots | 100K | ~12.8 MB |
| Bytecode | 1K | Variable (up to 24KB per contract × 1K ≈ 24 MB) |
| **Total** | — | **~14-38 MB** |

The existing `ExecutionCache` already allows configurable total size.

## 5. Risk Analysis

### Cache invalidation correctness

| Scenario | Current handling in `ExecutionCache` |
|----------|--------------------------------------|
| Normal block execution | `insert_state(&BundleState)` overwrites touched entries |
| Reorg to different fork | `SavedCache::clear()` if parent hash mismatch |
| `SELFDESTRUCT` (pre-Dencun) | Clears entire account + storage cache |
| RPC reads concurrent with execution | NOT handled — cache is engine-only today |

### Key risks of sharing the cache with RPC

1. **Stale reads during block execution**: If RPC reads from the cache while a block is mid-execution, it could see partially-updated state. Mitigation: epoch tagging in `fixed-cache` + only expose the cache *after* block commit.

2. **Cache pollution from RPC workloads**: RPC `eth_call` traces could evict hot engine entries. Mitigation: read-only access for RPC (no insertion on miss), or separate read-through cache.

3. **Memory pressure**: Adding RPC reads increases cache pressure but doesn't increase memory usage if the cache size is fixed.

4. **Consensus correctness**: The cache is advisory — every miss falls through to MDBX which is the source of truth. Cache corruption would cause performance degradation, not consensus failures, as long as reads always fall through on miss.

### Thread safety

`fixed-cache` is lock-free and designed for concurrent access. `DashMap` uses sharded locks. Both are safe for multi-threaded use. The `SavedCache` lifecycle management (via `Arc<()>` guards) ensures exclusive mutation access.

## 6. Conclusion

**Reth already has a sophisticated hot state cache** in `ExecutionCache` / `CachedStateProvider`. The gap is:

1. **RPC reads don't benefit from it** — the cache lives in the engine tree and is not exposed to the RPC provider layer
2. **The cache is tied to the `PayloadProcessor` lifecycle** — it's created/destroyed with the payload execution context

The highest-value next step would be to:
1. Make the `ExecutionCache` (or a read-only view of it) accessible to `LatestStateProviderRef` when serving RPC requests
2. Or create a second, read-through `schnellru`-based cache at the `LatestStateProviderRef` level that serves RPC reads, sized at ~10K accounts + ~100K storage slots (~15 MB)

Either approach avoids touching consensus-critical code — the cache is purely advisory, and all misses fall through to MDBX.
