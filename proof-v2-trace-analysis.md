# Proof V2 Trace Analysis - Block 24275866

## Test Configuration
- **Baseline**: `--engine.account-storage-worker-count=0` (legacy proofs)
- **Feature**: `--engine.enable-proof-v2 --engine.account-storage-worker-count=0` (v2 proofs)
- **Block range**: 24275842 - 24275871 (30 blocks)

## Benchmark Summary
| Metric | Baseline | Feature | Change |
|--------|----------|---------|--------|
| Mean latency | 29.99ms | 32.27ms | +7.6% |
| P50 latency | 27.96ms | 29.25ms | +4.6% |
| P90 latency | 43.34ms | 43.04ms | -0.7% |
| **P99 latency** | 55.16ms | 80.32ms | **+45.6%** |

## P99 Outlier Analysis (Block 24275866)

### Jaeger Trace URLs
- **Baseline**: http://dev-brian:16686/trace/2dc16019eee13c53bb9ebd0baae65c8e
- **Feature**: http://dev-brian:16686/trace/c1fa23401ac0b01cf839374e82b314e6

### Key Timing Differences

| Component | Baseline | Feature | Diff |
|-----------|----------|---------|------|
| **Total payload** | 36.6ms | 88.7ms | +52ms |
| spawn_sparse_trie_task | 44.7ms | 113.4ms | +69ms |
| storage worker max | 36.6ms | 95.1ms | +59ms |
| account worker max | 36.3ms | 91.4ms | +55ms |
| await_state_root | 1.8ms | 42.0ms | +40ms |
| update_sparse_trie max | 3.1ms | 26.0ms | +23ms |
| storage_trie max | 1.2ms | 11.1ms | +10ms |
| account_trie max | 0.9ms | 10.3ms | +9ms |
| recv tx total | 0.13ms | 8.4ms | +65x |

### Proof Calculation Comparison

| Metric | Baseline | Feature |
|--------|----------|---------|
| Storage proof count | 893 | 885 |
| Storage proof total time | 95.5ms | 88.9ms |
| Storage proof avg | 0.107ms | 0.100ms |
| Account proof count | 117 | 122 |

Individual proof calculations are **similar** between baseline and feature.

### Root Cause Analysis

1. **Workers running 2.5x longer** despite similar proof calculation counts:
   - Baseline storage worker max: 36.6ms (4.7ms traced, 32ms untraced)
   - Feature storage worker max: 95.1ms (5.3ms traced, **90ms untraced**)

2. **Channel receive overhead** (`recv tx` spans):
   - Baseline: 269 calls, 0.13ms total, 0.004ms max
   - Feature: 269 calls, 8.4ms total, 1.7ms max
   - **65x slowdown** suggests channel contention

3. **await_state_root blocked**:
   - Baseline: 1.8ms
   - Feature: 42.0ms
   - Sparse trie updates waiting on proof workers

4. **Missing "Waiting for storage proof" spans** in feature:
   - Baseline: 842 spans (37ms total)
   - Feature: 0 spans
   - V2 proof path handles storage proofs differently

5. **Sparse trie update slowdown**:
   - Baseline: 25 updates, 3.1ms max, 31.7ms total
   - Feature: 11 updates, 26.0ms max, 76.3ms total
   - Fewer but much longer updates in v2

## Hypothesis

With `--engine.account-storage-worker-count=0`, storage roots are computed **synchronously** on the critical path. The v2 proof system has higher overhead when workers can't parallelize storage root computation:

1. Workers block longer computing storage proofs inline
2. Sparse trie task waits longer for proofs to complete
3. Channel contention increases as updates queue up
4. The 90ms untraced busy time in workers is likely the inline storage root calculation

## Recommendations

1. **Test with account-storage-workers enabled** to see if v2 performs better with parallelization
2. **Add tracing** to the untraced worker code paths to identify the exact bottleneck
3. **Profile the storage root computation** in the v2 path to understand the 3x overhead
