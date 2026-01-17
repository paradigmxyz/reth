# Reth/Tempo Performance Optimization Opportunities

Based on #eng-perf and #ai-agent Slack channel analysis (Jan 2026).

## Current Bottleneck Analysis

From profiling sessions, the persistence phase breakdown is:

| Component | % Time | Priority |
|-----------|--------|----------|
| `update_history_indices` | 26.0% | **Critical** |
| `write_trie_updates` | 25.4% | High |
| `write_trie_changesets` | 24.2% | High |
| `write_state` | 13.8% | Medium |
| `write_hashed_state` | 10.6% | Medium |

## Critical Optimizations

### 1. History Indices from In-Memory Data (26% improvement potential)

**Problem**: Currently scans `AccountChangeSets`/`StorageChangeSets` tables via DB cursors.

**Solution**: Derive transitions directly from in-memory `ExecutionOutcome`:

```rust
// Before the loop, accumulate transitions:
let mut account_transitions: BTreeMap<Address, Vec<u64>> = BTreeMap::new();
let mut storage_transitions: BTreeMap<(Address, B256), Vec<u64>> = BTreeMap::new();

// Inside the per-block loop, extract from execution_outcome.bundle.reverts
for (block_idx, block_reverts) in execution_output.bundle.reverts.iter().enumerate() {
    let block_number = execution_output.first_block() + block_idx as u64;
    for (address, account_revert) in block_reverts {
        account_transitions.entry(*address).or_default().push(block_number);
        for storage_key in account_revert.storage.keys() {
            let key = B256::new(storage_key.to_be_bytes());
            storage_transitions.entry((*address, key)).or_default().push(block_number);
        }
    }
}

// After loop, replace update_history_indices(range) with:
self.insert_account_history_index(account_transitions)?;
self.insert_storage_history_index(storage_transitions)?;
```

**Benchmark**: `cargo bench -p reth-engine-tree --bench heavy_persistence -- history_indices`

### 2. Batch Trie Writes Across Blocks (~50% improvement for b2b)

**Problem**: `write_trie_changesets` and `write_trie_updates_sorted` called per-block.

**Solution**: Accumulate overlay across blocks, write once at end:

```rust
let mut trie_overlay = TrieUpdatesSorted::default();

for block in blocks {
    self.write_trie_changesets(block_number, &trie_data.trie_updates, Some(&trie_overlay))?;
    trie_overlay.extend(&trie_data.trie_updates);
}
// Single write at end
self.write_trie_updates_sorted(&trie_overlay)?;
```

**Benchmark**: `cargo bench -p reth-engine-tree --bench heavy_persistence -- accumulated`

### 3. O(N²) Overlay Merge Fix (PR #20774)

**Problem**: `wait_cloned()` iterates through ALL ancestors for each block → O(N²) complexity.

**Results achieved**:
- p50: -8.40%
- p90: -42.46%
- p99: -60.30%
- Gas/Second: +73.34%

**Benchmark**: `cargo bench -p reth-engine-tree --bench heavy_persistence -- overlay_merge`

## High Priority Optimizations

### 4. MDBX Configuration Tuning

**Problem**: msync taking seconds on new reth boxes (99% of samples in `mdbx_txn_commit_ex`).

**Potential solutions**:
- Expose `txn_dp_limit` - forces dirty pages to spill during transaction
- Expose `sync_bytes`/`sync_period` - triggers intermediate flushes
- Lower `spill_max_denominator` for more aggressive spilling

```rust
// Already in libmdbx-rs but not exposed to CLI
mdbx_env_set_syncbytes(env, 100*1024*1024);  // Flush every 100MB
mdbx_env_set_syncperiod(env, 16384);         // Or every ~0.25s
```

### 5. Execution Cache Improvements

**Problem**: moka cache contention under high throughput, expensive cache misses.

**Findings**:
- TIP-20 transfers trigger mostly cache misses (unique accounts)
- fixed_cache (4GB) allocation overhead when cache misses
- Pre-warming effectiveness varies

**Benchmark**: `cargo bench -p reth-engine-tree --bench execution_cache`

**Key metrics to track**:
- Cache hit rate (baseline 45%, target 78%+ like Half-Path)
- Contention under 8-32 threads
- Burst insert latency

### 6. Parallel State Root Scaling

**Problem**: State root calculation is 50-80% of validation time for large blocks.

**Benchmark scenarios**:
- Normal block: 3,000 accounts
- Heavy block: 10,000 accounts  
- Megablock (1.5 GGas): 30,000+ accounts

**Benchmark**: `cargo bench -p reth-trie-parallel --bench heavy_root`

## Medium Priority Optimizations

### 7. Pre-sort Storage Tries Once

Currently sorted every call at multiple locations:
```rust
let mut storage_updates = trie_updates.storage_tries_ref().iter().collect::<Vec<_>>();
storage_updates.sort_unstable_by(|a, b| a.0.cmp(b.0));
```

Could use `BTreeMap` internally or pre-sorted Vec in `TrieUpdatesSorted`.

### 8. Remove Expensive HashMap Clone

Recent change introduced a clone showing up in profiles (3s block profile).

## Benchmarking Commands

```bash
# Run all heavy benchmarks on an idle box
./scripts/bench-heavy.sh ./results-$(date +%Y%m%d)

# Run specific benchmark groups
cargo bench -p reth-engine-tree --bench execution_cache
cargo bench -p reth-engine-tree --bench heavy_persistence
cargo bench -p reth-trie-parallel --bench heavy_root

# Compare against baseline
cargo bench -p reth-engine-tree --bench execution_cache -- --baseline heavy-cache

# Profile with samply
samply record -- cargo bench -p reth-engine-tree --bench execution_cache -- cache/contention
```

## Profiling Resources

- [How to benchmark and profile Reth](https://www.notion.so/How-to-benchmark-and-profile-Reth-21532f2c34848058a2f6efc5f852603d)
- [Perf onboarding doc](https://docs.google.com/document/d/1pgbWk6wjd3p3oGy2SC2mWiGAvzlcIoiB-8g20fm6Acc)
- Firefox Profiler compare: https://profiler.firefox.com/compare/

## Related PRs

- [#20774](https://github.com/paradigmxyz/reth/pull/20774) - Overlay reuse optimization
- [#20616](https://github.com/paradigmxyz/reth/pull/20616) - Subscribe to persisted block
- [#20520](https://github.com/paradigmxyz/reth/pull/20520) - fixed_cache execution cache
- [#20405](https://github.com/paradigmxyz/reth/pull/20405) - Defer transaction pool notifications
- [#20398](https://github.com/paradigmxyz/reth/pull/20398) - Use RwLock for transaction pool listeners
