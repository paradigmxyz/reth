# TrieDB vs MDBX State Root Performance

This document compares the performance of TrieDB and MDBX for state root calculation.

## Benchmark Setup

- **Base state**: 100,000 accounts with 10 storage slots each
- **Overlay sizes**: 1,000 and 10,000 account updates
- **Sample size**: 10 iterations per benchmark

## Running the Benchmark

```bash
cargo bench -p reth-provider --features "test-utils,triedb" --bench bench_state_root_overlay
```

View results:
```bash
open target/criterion/overlay_1000/report/index.html
open target/criterion/overlay_10000/report/index.html
```

## Results

### Overlay Size: 1,000 accounts

| Implementation | Mean      | Range                   | Speedup |
|----------------|-----------|-------------------------|---------|
| MDBX           | 416.58 ms | [413.75 ms - 419.56 ms] | 1x      |
| TrieDB         | 3.17 ms   | [3.15 ms - 3.19 ms]     | **131x**|

![Violin Plot - 1,000 accounts overlay](images/violin_overlay_1000.svg)

### Overlay Size: 10,000 accounts

| Implementation | Mean      | Range                   | Speedup |
|----------------|-----------|-------------------------|---------|
| MDBX           | 425.56 ms | [422.67 ms - 428.68 ms] | 1x      |
| TrieDB         | 37.45 ms  | [37.10 ms - 37.92 ms]   | **11x** |

![Violin Plot - 10,000 accounts overlay](images/violin_overlay_10000.svg)

## Analysis

- **TrieDB is dramatically faster** for state root calculation with overlay states
- At 1,000 account overlay: TrieDB is ~131x faster than MDBX
- At 10,000 account overlay: TrieDB is ~11x faster than MDBX
- MDBX performance is relatively constant regardless of overlay size (~417-426ms)
- TrieDB scales linearly with overlay size (3.2ms -> 37.5ms for 10x more accounts)
- TrieDB's advantage is most pronounced with smaller overlays

## Environment

- Base state: 100,000 accounts with 10 storage slots each
- Sample size: 10 iterations per benchmark
