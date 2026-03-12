# APST Parallelism Threshold Benchmarks — Implementation Plan

## Goal

Determine optimal values for the four `ArenaParallelismThresholds` fields by measuring
wall-clock time of each APST phase across a matrix of dataset sizes, changeset sizes,
and threshold values.

## Thresholds Under Test

| Field | Phase | Default |
|---|---|---|
| `min_revealed_nodes` | `reveal_nodes` | 16 |
| `min_updates` | `update_leaves` | 128 |
| `min_dirty_leaves` | `update_subtrie_hashes` (via `root()`) | 64 |
| `min_leaves_for_prune` | `prune` | 128 |

## Key Design Decisions

1. **Uniform random keys are realistic.** Ethereum trie keys are keccak hashes of
   addresses/slots, so `B256` drawn from a uniform distribution faithfully
   represents real trie shape.

2. **Phases are independent.** In production (`SparseTrieCacheTask`), each phase
   (reveal → update → hash → prune) runs as an atomic unit across all tries.
   Storage tries and the account trie run in parallel _within_ a phase via rayon,
   but the phases themselves are sequential. We therefore benchmark each phase in
   isolation.

3. **No separate "build APST" step.** Each benchmark function starts from a
   default APST, runs the full reveal → update → hash → prune pipeline, but only
   _measures_ the phase under test. The other phases are setup/teardown.

4. **Sweep one threshold at a time.** The other three thresholds are set to 0 (always
   parallel) during measurement so we isolate the effect of the target threshold.
   A secondary validation run can test all-at-defaults to confirm no interactions.

## Benchmark Matrix

### Axes

| Axis | Values |
|---|---|
| `dataset_size` | 100, 1_000, 10_000, 50_000 |
| `changeset_size` | 50, 500, 5_000, 25_000 (capped at dataset_size) |
| `target_threshold` | one of the four fields above |
| `threshold_value` | 1, 4, 16, 32, 64, 128, 256, 512, 1024 |

This gives ~4 × 4 × 4 × 9 = 576 benchmark cases total (minus invalid combos where
changeset > dataset). Each case runs for criterion's default measurement time.

### Grouping

Use `criterion::BenchmarkGroup` per `(target_threshold, dataset_size)` pair with
`BenchmarkId::new(format, (changeset_size, threshold_value))` so the HTML report
shows throughput curves for each phase/dataset combo.

## Data Generation

Reuse the pattern from the APST proptest / `TrieTestHarness`:

```
fn generate_bench_data(dataset_size, changeset_size, seed) -> BenchData:
    1. Generate `dataset_size` random B256 keys with non-zero U256 values.
    2. Build a TrieTestHarness from this dataset.
    3. Sample `changeset_size` changeset entries:
       - 70% updates to existing keys (new random non-zero values)
       - 20% new key insertions
       - 10% deletions of existing keys (U256::ZERO)
    4. Pre-compute proof targets and proofs needed for reveal.
    5. Return everything needed to run the pipeline.
```

Use a **fixed seed** per `(dataset_size, changeset_size)` pair so the dataset is
identical across threshold values — the only variable is the threshold.

## Benchmark Function Structure

Each benchmark function follows this template:

```rust
// For benching the "update_leaves" phase:
b.iter_batched(
    || {
        // SETUP: clone pre-built APST state that has reveal already done
        setup_data.clone()
    },
    |mut data| {
        // TIMED: only the phase under test
        apst.update_leaves(&mut data.leaf_updates, |_, _| {});
    },
    BatchSize::SmallInput,
);
```

### Per-Phase Setup

| Phase | Setup (untimed) | Measured |
|---|---|---|
| `reveal_nodes` | Create default APST, set root | `reveal_nodes(&mut proof_nodes)` |
| `update_leaves` | Create APST, set root, reveal all proofs | `update_leaves(...)` |
| `update_subtrie_hashes` / `root` | Create APST, set root, reveal, update leaves | `root()` |
| `prune` | Create APST, set root, reveal, update, hash | `prune(&retained)` |

## File Layout

```
crates/trie/sparse/
├── Cargo.toml              # add criterion + [[bench]] section
└── benches/
    └── apst_thresholds.rs  # single benchmark file
```

## Analysis

After running:

```bash
cargo bench -p reth-trie-sparse --bench apst_thresholds
```

Criterion generates HTML reports under `target/criterion/`. For each
`(phase, dataset_size)` group, look at the throughput curve as threshold_value
increases. The optimal threshold is the inflection point where going lower
(more parallelism) stops improving or starts hurting throughput due to rayon
scheduling overhead.
