# Overlay State Provider Performance Analysis

## 🚨 Critical Finding

**600-700ms spikes in Overlay State Provider are NOT reth-bench specific** - they occur in production during Engine API block validation!

## 📊 Root Cause

### Where block_hash is Set (Production Code)

Found in `crates/engine/tree/src/tree/payload_validator.rs:655` and `:785`:

```rust
let factory = OverlayStateProviderFactory::new(self.provider.clone())
    .with_block_hash(Some(block_hash))  // ← Sets block_hash to parent block
    .with_trie_overlay(Some(multiproof_config.nodes_sorted))
    .with_hashed_state_overlay(Some(multiproof_config.state_sorted));
```

**Context**: This is in the **payload validator** that validates blocks received from the consensus layer via Engine API.

**Why block_hash is set**: The validator needs state at the parent block to validate the new block's state transitions.

### Why Checkpoint Lags

**MerkleChangeSets stage** (`crates/stages/stages/src/stages/merkle_changesets.rs`):
- Runs as part of the **staged sync pipeline**
- Only updates when **MerkleExecute stage** completes
- Processes blocks in **batches** based on finalized blocks or retention window (default 64 blocks)
- Not real-time - runs periodically as part of sync

**The Problem**:
1. New blocks are processed by Engine API → Execution stage
2. MerkleChangeSets stage lags behind MerkleExecute
3. Checkpoint can be 45-74 blocks behind current tip
4. Overlay provider needs reverts when `requested_block > checkpoint`
5. **600ms spent fetching reverts** for 45-74 blocks of history

## 🎯 Impact Analysis

### Metrics Summary
- **Frequency**: 1.72 req/s (0.4% of overlay provider calls)
- **Cost per call**: 600-700ms
  - Trie Reverts: 500-600ms
  - State Reverts: 60-120ms
- **Total overhead**: ~1 second of DB queries per second
- **Throughput correlation**: One observed 57% drop (14:40), but not consistent

### What's NOT Affected
✅ **State root computation**: Stays 5-10μs (completely unaffected)
✅ **95%+ of overlay calls**: Use fast path (no reverts)
✅ **Block processing**: No direct correlation with spikes

### What IS Affected
⚠️ **Engine API validation**: When checkpoint lags, validation requires expensive reverts
⚠️ **Database load**: 500-600ms of read queries per affected call
⚠️ **Tail latency**: p99 throughput can drop during revert fetches

## 🔍 Why This Happens

```
Timeline of Events:

14:40:00 ──────────────────────────────────────────────────────► 14:46:00
    │                                                               │
    ├─ Engine API receives new block (parent = block N)           │
    │  - Sets block_hash = N (parent block)                        │
    │  - Checkpoint at block N-60 (lagging!)                       │
    │  - Needs reverts: N-60 → N (60 blocks!)                      │
    │  - Trie fetch: 500ms for 60 blocks                           │
    │  - State fetch: 60ms for 60 blocks                           │
    │  - Total: 600ms spike                                        │
    │                                                               │
    ├─ MerkleChangeSets runs (periodic)                            │
    │  - Updates checkpoint to block N                             │
    │  - Next validation: no reverts needed!                       │
    │                                                               │
    └─ Process repeats every ~70 blocks                            │
```

## 🛠️ Solution Options

### Option 1: Run MerkleChangeSets More Frequently ⭐ RECOMMENDED

**Change**: Reduce batch size from current (probably 64+ blocks) to smaller batches (10-20 blocks)

**How**:
1. Find MerkleChangeSets configuration in pipeline setup
2. Reduce `retention_blocks` or add time-based trigger
3. Make stage run every 10-20 blocks instead of 64+

**Pros**:
- ✅ Reduces checkpoint lag to 10-20 blocks
- ✅ Reduces revert fetch time to ~100-200ms (vs 600ms)
- ✅ More consistent performance

**Cons**:
- ⚠️ More frequent checkpoint writes (increased DB I/O)
- ⚠️ Need to measure impact on overall throughput

**Implementation**:
```rust
// In stage pipeline configuration
MerkleChangeSets::with_retention_blocks(20)  // Was: 64
```

### Option 2: Optimize Revert Fetching

**Change**: Cache recent reverts or make fetching async

**A. LRU Cache**:
```rust
struct OverlayStateProviderFactory<F> {
    factory: F,
    revert_cache: Arc<Mutex<LruCache<(BlockNumber, BlockNumber), CachedReverts>>>,
}
```

**B. Async Fetching**:
```rust
// Don't block overlay creation - fetch reverts in background
let revert_future = tokio::spawn(async move {
    provider.trie_reverts(from_block + 1)
});
```

**Pros**:
- ✅ Reduces blocking time
- ✅ Can help with repeated queries

**Cons**:
- ⚠️ Adds complexity
- ⚠️ Cache may not help much (queries are for different ranges)
- ⚠️ Async doesn't reduce actual DB query time

### Option 3: Accept Current Behavior ⚠️

**If**:
- 0.4% of calls taking 600ms is acceptable
- Throughput impact is minimal
- State root (critical path) is unaffected

**Then**: Document as expected behavior, add monitoring/alerting

### Option 4: Optimize MerkleChangeSets Stage Itself

**Change**: Make the stage itself faster so it can keep up

**How**:
- Profile `HashedPostState::from_reverts()` (line 195-198)
- Optimize trie update calculations (line 234-253)
- Parallelize block processing if possible

**Pros**:
- ✅ Benefits all operations, not just overlay
- ✅ Reduces overall sync time

**Cons**:
- ⚠️ Most complex solution
- ⚠️ May have limited optimization potential

## 📈 Additional Metrics Needed

### 1. MerkleChangeSets Performance
Add to `crates/stages/stages/src/stages/merkle_changesets.rs`:

```rust
#[cfg(feature = "metrics")]
use reth_metrics::{metrics::{Counter, Histogram, Gauge}, Metrics};

#[derive(Metrics)]
#[metrics(scope = "stages.merkle_changesets")]
struct MerkleChangeSetsMetrics {
    /// Time to execute stage
    execution_duration: Histogram,
    /// Blocks processed per execution
    blocks_per_execution: Histogram,
    /// Current checkpoint block
    checkpoint_block: Gauge,
    /// Checkpoint lag (tip - checkpoint)
    checkpoint_lag: Gauge,
}
```

**Add instrumentation**:
```rust
fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
    #[cfg(feature = "metrics")]
    let _timer = start_timer(&self.metrics.execution_duration);

    let target_range = self.determine_target_range(provider)?;
    let blocks_count = target_range.end - target_range.start;

    #[cfg(feature = "metrics")]
    self.metrics.blocks_per_execution.record(blocks_count as f64);

    // ... rest of execution

    #[cfg(feature = "metrics")]
    {
        self.metrics.checkpoint_block.set(checkpoint as f64);
        let tip = provider.best_block_number()?;
        self.metrics.checkpoint_lag.set((tip - checkpoint) as f64);
    }
}
```

### 2. Overlay Usage Tracking
Add to `crates/engine/tree/src/tree/payload_validator.rs`:

```rust
#[cfg(feature = "metrics")]
use reth_metrics::metrics::Counter;

// Track when overlay is created with block_hash
#[cfg(feature = "metrics")]
static OVERLAY_WITH_BLOCK_HASH: Counter =
    Counter::new("engine_payload_validator_overlay_with_block_hash");

// Before creating factory:
#[cfg(feature = "metrics")]
OVERLAY_WITH_BLOCK_HASH.increment(1);
```

### 3. Grafana Alerts

```yaml
# Checkpoint lag alert
- alert: MerkleChangeSetsLagging
  expr: |
    (reth_sync_checkpoint{stage="MerkleChangeSets"}
     - reth_best_block_number) > 100
  for: 5m
  annotations:
    summary: "Checkpoint >100 blocks behind"

# High revert rate alert
- alert: OverlayRevertsFrequent
  expr: |
    rate(reth_storage_overlay_state_provider_reverts_required[5m]) > 5
  for: 5m
  annotations:
    summary: "Overlay reverts at {{ $value }} req/s"
```

## 🎯 Recommended Action Plan

### Phase 1: Add Metrics (Week 1)
1. ✅ Add MerkleChangeSets stage metrics
2. ✅ Add checkpoint lag gauge
3. ✅ Add overlay usage tracking
4. ✅ Deploy and collect baseline data

### Phase 2: Quick Win (Week 2)
5. 🎯 **Reduce MerkleChangeSets batch size** from 64 to 20 blocks
6. 📊 Measure impact:
   - Checkpoint lag should drop to 0-20 blocks
   - Revert duration should drop to ~100-200ms
   - Monitor overall throughput for regression

### Phase 3: Optimize (Week 3-4, if needed)
7. If Phase 2 insufficient:
   - Profile MerkleChangeSets execution
   - Consider async revert fetching
   - Consider LRU cache for recent ranges

### Phase 4: Production Validation (Week 4)
8. Compare metrics in production vs test
9. Validate solution works under real load
10. Document final performance characteristics

## 📝 Key Takeaways

1. ✅ **Root cause identified**: MerkleChangeSets checkpoint lags 45-74 blocks
2. ⚠️ **Not reth-bench specific**: Happens in production Engine API validation
3. ✅ **Impact is measurable but limited**: 0.4% of calls, doesn't affect state root
4. 🎯 **Solution is clear**: Run MerkleChangeSets more frequently
5. 📊 **Need better observability**: Add stage performance metrics

## 🔗 Related Code Locations

- Overlay provider: `crates/storage/provider/src/providers/state/overlay.rs`
- Overlay metrics: `crates/storage/provider/src/providers/state/overlay_metrics.rs`
- Payload validator: `crates/engine/tree/src/tree/payload_validator.rs:655,785`
- MerkleChangeSets stage: `crates/stages/stages/src/stages/merkle_changesets.rs`
- Dashboard: `dashboard.json` (panels 303-311)
