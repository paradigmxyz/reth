# TrieDB Performance Report

## 1. Introduction

Base has its own triedb (TDB) which is built directly on mmap disk. See [github.com/base/triedb](https://github.com/base/triedb). While MDBX is a general key-value store (whose page structure is a binary tree), TDB's page design is optimized for MPT (Merkle Patricia Trie) structure.

### Where Reth Interacts with TrieDB

1. **During init_genesis**: Reth calculates state root in memory and collects trie nodes, then persists them asynchronously in batches. TDB performance is poor in this stage. See [issue #179](https://github.com/base/triedb/issues/179).

2. **During payload validation**: Uses `StateRootTask` to recalculate the state root with multi-threaded proof revealing. This is only important for RPC nodes, as sequencers skip payload validation.

3. **During block production**: Calculates state root after EVM execution. This is the core logic affecting block production performance and is the focus of this benchmark.

The integration of TDB into reth can be found at [github.com/okx/reth/pull/78](https://github.com/okx/reth/pull/78).

---

## 2. Microbenchmark: State Root Calculation

### Setup

- **Base state**: 100,000 accounts with 10 storage slots each
- **Overlay sizes**: 1,000 and 10,000 account updates
- **Sample size**: 10 iterations per benchmark

### Running the Benchmark

```bash
cargo bench -p reth-provider --features "test-utils,triedb" --bench bench_state_root_overlay
```

View results:
```bash
open target/criterion/overlay_1000/report/index.html
open target/criterion/overlay_10000/report/index.html
```

### Results

#### Overlay Size: 1,000 accounts

| Implementation | Mean      | Range                   | Speedup |
|----------------|-----------|-------------------------|---------|
| MDBX           | 416.58 ms | [413.75 ms - 419.56 ms] | 1x      |
| TrieDB         | 3.17 ms   | [3.15 ms - 3.19 ms]     | **131x**|

![Violin Plot - 1,000 accounts overlay](images/violin_overlay_1000.svg)

#### Overlay Size: 10,000 accounts

| Implementation | Mean      | Range                   | Speedup |
|----------------|-----------|-------------------------|---------|
| MDBX           | 425.56 ms | [422.67 ms - 428.68 ms] | 1x      |
| TrieDB         | 37.45 ms  | [37.10 ms - 37.92 ms]   | **11x** |

![Violin Plot - 10,000 accounts overlay](images/violin_overlay_10000.svg)

### Analysis

- **TrieDB is dramatically faster** for state root calculation with overlay states
- At 1,000 account overlay: TrieDB is ~131x faster than MDBX
- At 10,000 account overlay: TrieDB is ~11x faster than MDBX
- MDBX performance is relatively constant regardless of overlay size (~417-426ms)
- TrieDB scales linearly with overlay size (3.2ms -> 37.5ms for 10x more accounts)
- TrieDB's advantage is most pronounced with smaller overlays

---

## 3. Real-World Benchmark: Block Production

### Benchmark Configuration

- **Commit**: 35add05ef334580588819474127d9ae9e0dfcc2b
- **Block gas limit**: 500M
- **Block interval**: 1s
- **PERSISTENCE_THRESHOLD**: 2 (default)

### Hardware

- **Chip**: Apple M4 Pro
- **Memory**: 48 GB

### Test Data

| Dataset | EOA Accounts | Contract Accounts | Storage per Contract | Genesis Size |
|---------|--------------|-------------------|----------------------|--------------|
| Small   | 2,000,000    | 500,000           | 10                   | ~1 GB        |
| Medium  | 8,000,000    | 2,000,000         | 40                   | ~3 GB        |

### Method

Launch an op-node with randomly created data and run ERC20 transfer transaction spam using [xlayer-toolkit/tools/adventure](https://github.com/okx/xlayer-toolkit/tree/main/tools/adventure). The tool deploys an ERC20 contract and creates 2000 EOA accounts that randomly send ERC20 tokens. The max pending transactions in pool is kept at 50k.

> **Note**: If benchmarking with an empty dataset, TDB's performance is worse compared to the original version because MDBX binary tree lookup overhead is minimized with small datasets.

### Results

#### Small Data (1GB genesis)

| Metric | Without TDB | With TDB | Improvement |
|--------|-------------|----------|-------------|
| **TPS** | 9,283 | 11,194 | **+20.58%** |
| **State Root (avg)** | 414ms | 130ms | **3.18x faster** |
| **State Root (max)** | 615ms | 137ms | - |

#### Medium Data (3GB genesis)

| Metric | Without TDB | With TDB | Improvement |
|--------|-------------|----------|-------------|
| **TPS** | 8,335 | 10,324 | **+23.86%** |
| **State Root (avg)** | 904ms | 144ms | **6.27x faster** |
| **State Root (max)** | 1.44s | 157ms | - |

---

## 4. Analysis: PERSISTENCE_THRESHOLD Impact

The `PERSISTENCE_THRESHOLD` controls after how many executed blocks the output is flushed to the database. We analyzed its impact on performance with small data (1GB genesis).

### Results by PERSISTENCE_THRESHOLD

| Threshold | Without TDB | | With TDB | | TPS Improvement | State Root Speedup |
|-----------|-------------|---------------|----------|---------------|-----------------|-------------------|
| | TPS | State Root | TPS | State Root | | |
| **2** (default) | 9,283 | 414ms | 11,194 | 130ms | **+20.6%** | **3.18x** |
| **8** | 9,062 | 468ms | 10,367 | 137ms | **+14.4%** | **3.42x** |
| **32** | 7,721 | 655ms | 10,759 | 148ms | **+39.4%** | **4.43x** |
| **128** | 8,283 | 657ms | 9,854 | 183ms | **+19.0%** | **3.59x** |
| **512** | 7,172 | 756ms | 8,798 | 268ms | **+22.7%** | **2.82x** |

### Key Findings

1. **Without TDB**: TPS degrades as threshold increases (9,283 → 7,172), and state root time increases significantly (414ms → 756ms). This is because `TrieUpdates::extend_ref` takes more time as it iterates over more block trie updates.

2. **With TDB**: TPS remains relatively stable (11,194 → 8,798), and state root time increases modestly (130ms → 268ms). TDB merges plain state instead of extending trie nodes, which is much faster.

3. **Optimal Setting**: The default `PERSISTENCE_THRESHOLD=2` with TDB enabled achieves the highest TPS (11,194) and fastest state root calculation (130ms).

---

## 5. CPU Profiling Analysis

To validate the hypothesis that `TrieUpdates::extend` functions are the bottleneck when `PERSISTENCE_THRESHOLD` increases, we conducted CPU profiling using Linux `perf`.

### Profiling Setup

- **Tool**: Linux perf with flamegraph visualization
- **Duration**: 60 seconds per profile
- **Comparison**: threshold=2 vs threshold=32 (without TDB)
- **Workload**: ERC20 transfer spam on 1GB genesis

### Profiling Results

#### Trie Update Extension Functions (The Bottleneck)

| Function | Threshold=2 | Threshold=32 | Increase |
|----------|-------------|--------------|----------|
| `TrieUpdates::extend_from_sorted` | 1.42B samples (1.8%) | 8.18B samples (11.5%) | **5.75x** |
| `StorageTrieUpdates::extend_from_sorted` | 179M samples | 1.09B samples | **6.1x** |
| `HashedPostState::extend_from_sorted` | 313M samples | 2.29B samples | **7.3x** |

> Note: B = Billion samples, M = Million samples. More samples = more CPU time.

#### State Root Calculation Functions

| Function | Threshold=2 | Threshold=32 | Change |
|----------|-------------|--------------|--------|
| `StateRoot::calculate` | 17.2B samples | 13.2B samples | **-23%** (faster) |
| `overlay_root_from_nodes_with_updates` | 17.3B samples | 13.2B samples | **-23%** (faster) |

### Analysis

The profiling confirms the hypothesis:

1. **The bottleneck is in merging trie updates, not in state root calculation itself.**
   - `extend_from_sorted` functions show 5.75x - 7.3x increase in CPU time
   - These functions merge trie updates from multiple cached blocks

2. **State root calculation is actually faster at higher thresholds.**
   - `StateRoot::calculate` is 23% faster at threshold=32
   - This is expected: more trie nodes cached in memory → fewer disk reads

3. **The merging overhead outweighs the cache benefit.**
   - With threshold=32, you're merging updates from 32 blocks instead of 2
   - Each `extend_from_sorted` call iterates over more data

### Why TDB Avoids This Problem

TDB merges **plain state** (account addresses and storage keys) instead of **trie nodes**:

| Approach | Data to Merge | Complexity |
|----------|---------------|------------|
| Without TDB | Pre-computed trie nodes (branch nodes, extension nodes, leaf nodes) | O(n) where n = total trie nodes across all cached blocks |
| With TDB | Plain state (addresses, storage keys) | O(m) where m = unique accounts/storage modified |

Plain state is much smaller and simpler to merge than trie node structures, which is why TDB maintains stable performance even at higher thresholds.

---

## 6. Summary

### Performance Improvements with TrieDB

| Dataset | TPS Improvement | State Root Speedup |
|---------|-----------------|-------------------|
| Small (1GB) | +20.58% | 3.18x |
| Medium (3GB) | +23.86% | 6.27x |

### Key Takeaways

- **TrieDB provides significant performance improvements** for block production on sequencer nodes
- **Larger datasets benefit more** from TrieDB (6.27x speedup on 3GB vs 3.18x on 1GB)
- **TDB maintains stable performance** even with higher PERSISTENCE_THRESHOLD values, where the original implementation degrades significantly
- **Microbenchmarks show up to 131x speedup** for isolated state root calculations
- **CPU profiling confirms** the bottleneck is in `TrieUpdates::extend_from_sorted` functions, which scale poorly with higher thresholds
- **Recommended configuration**: `PERSISTENCE_THRESHOLD=2` (default) with TDB enabled
