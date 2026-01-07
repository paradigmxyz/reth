# extend_sorted_vec Benchmark Results

Comparison of old (iterate+peek+collect+sort) vs new (two-pointer merge) implementation.

## Algorithm Comparison

| Implementation | Complexity | Description |
|----------------|------------|-------------|
| **Old** | O((n+m) log(n+m)) | Iterate target, peek other, collect inserts, then sort |
| **New** | O(n+m) | Two-pointer merge, no sorting needed |

## Results

### Single Extend Operations

| Target Size | Old Implementation | New Implementation | Speedup |
|-------------|-------------------|-------------------|---------|
| 100 | 1.26 µs | 345 ns | **3.6x** |
| 1,000 | 7.63 µs | 1.68 µs | **4.5x** |
| 10,000 | 84.4 µs | 15.5 µs | **5.4x** |
| 100,000 | 1.00 ms | 141 µs | **7.1x** |

### Cumulative Extend (simulates deferred trie overlay growth)

| Blocks | Old Implementation | New Implementation | Speedup |
|--------|-------------------|-------------------|---------|
| 10 | 36.5 µs | 8.5 µs | **4.3x** |
| 50 | 1.21 ms | 205 µs | **5.9x** |
| 100 | 4.64 ms | 767 µs | **6.0x** |

## Speedup Visualization

```
Single Extend Speedup by Target Size
────────────────────────────────────
100     │████████████████████ 3.6x
1,000   │██████████████████████████ 4.5x
10,000  │████████████████████████████████ 5.4x
100,000 │██████████████████████████████████████████ 7.1x
        └────────────────────────────────────────────
         1x    2x    3x    4x    5x    6x    7x    8x

Cumulative Extend Speedup by Blocks
───────────────────────────────────
10      │█████████████████████████ 4.3x
50      │███████████████████████████████████ 5.9x
100     │████████████████████████████████████ 6.0x
        └────────────────────────────────────────────
         1x    2x    3x    4x    5x    6x    7x    8x
```

## Key Observations

1. **Speedup scales with size**: Larger targets show greater improvement (3.6x → 7.1x)
2. **Cumulative pattern**: 6x improvement for the realistic deferred trie use case
3. **Algorithm**: Simple two-pointer merge, O(n+m) vs O((n+m) log(n+m))

## Benchmark Environment

- **CPU**: See system info
- **Rust**: nightly
- **Criterion**: Default settings with 100 samples

## Running the Benchmark

```bash
cargo bench --bench extend_sorted_vec -p reth-trie-common
```
