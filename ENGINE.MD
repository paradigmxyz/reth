# Engine Stage Pipeline - Implementation Notes

## Request
Create a pipeline sync with headers, bodies, sender recovery, tx lookup, and
an "engine" stage. The engine stage executes downloaded transactions using the
same path as the engine during live sync. Persistence threshold: 5000 blocks.

## Architecture

### Pipeline Structure
```
HeaderStage -> BodyStage -> SenderRecoveryStage -> TransactionLookupStage -> EngineStage
```

5 stages. Sender recovery and transaction lookup are handled by the existing
reth stages. The EngineStage reads pre-recovered senders from DB and focuses
on EVM execution + state persistence.

### How EngineStage Works

1. **Read blocks with senders** from DB via `block_with_senders_range`
2. **Execute entire batch** through a single `batch_executor` backed by one
   DB state provider. The executor accumulates state changes internally in
   a `BundleState`, so each block sees the state from all prior blocks.
3. **Compute hashed state** once from the accumulated `BundleState`
4. **Persist** via the pipeline's write transaction using the same write
   methods that `save_blocks` calls internally (`write_state` with
   `OriginalValuesKnown::Yes`, `write_hashed_state`, `update_history_indices`,
   `update_pipeline_stages`)

### Batch Persistence
- `batch_size = 5000` (configurable)
- Returns `done = false` between batches → pipeline commits
- 100k blocks → 20 batches

## File Locations
- `crates/stages/stages/src/stages/engine_stage.rs` — EngineStage (~130 lines)
- `crates/stages/stages/src/stages/mod.rs` — module registration
- `crates/stages/stages/tests/engine_pipeline.rs` — 15k block test
- `crates/stages/stages/tests/bench_pipeline.rs` — benchmark comparison (100k blocks)

## Benchmark Results (release mode, 100k blocks, sequential runs, 32-core machine)

### Post-TxLookup stages ONLY (apples-to-apples comparison)

Prefix pipeline (H→B→SR→TxLookup) run as untimed setup. Only the execution +
state persistence stages are timed.

Default = Execution → AccountHashing → StorageHashing → Merkle → IndexHistory → Prune → Finish
Engine  = EngineStage (single stage replaces all of the above)

| Run | Default post-TxLookup | Engine post-TxLookup | Ratio |
|-----|----------------------|---------------------|-------|
| 1   | 949ms                | 820ms               | 0.86x |
| 2   | 900ms                | 744ms               | 0.83x |
| 3   | 936ms                | —                   | —     |

**EngineStage is ~15-17% faster than the equivalent default stages.**

### Full pipeline (including prefix stages)

| Run | Default Pipeline | Engine Pipeline | Ratio |
|-----|-----------------|-----------------|-------|
| 1   | 1.894s           | 1.759s           | 0.93x |
| 2   | 1.888s           | 1.771s           | 0.94x |
| 3   | 1.838s           | 1.751s           | 0.95x |

### Optimization history

| Version | vs Default | Pipeline |
|---------|------------|----------|
| v0      | 2.5x slower | H → B → Engine (sync persist, no overlay) |
| v1      | deadlock    | H → B → Engine (bg persist, MDBX deadlock) |
| v2      | 7.9x slower | H → B → Engine (per-block MemoryOverlay) |
| v3      | 2.4x slower | H → B → Engine (single batch_executor) |
| v4      | ~0.9x       | H → B → Engine (parallel recovery + OVK::Yes) |
| v5      | ~0.9x       | H → B → SR → Engine |
| **v6**  | **~0.94x**  | **H → B → SR → TxLookup → Engine** |

### Why EngineStage is faster

1. **Single stage** replaces 7+ stages: Execution, AccountHashing, StorageHashing,
   MerkleExecute, IndexAccountHistory, IndexStorageHistory, Finish. Fewer DB
   commits and checkpoint updates.
2. **`OriginalValuesKnown::Yes`**: Batch executor tracks original values via
   revm's `State<DB>`, skipping re-reads during changeset generation.
3. **Single executor**: One `State<DB>` accumulates all changes vs multiple
   stages each doing separate DB access patterns.
4. **Batch hashed state**: `HashedPostState::from_bundle_state` computed once
   per 5k-block batch from the accumulated BundleState, vs per-account/storage
   cursor walks in the hashing stages.
