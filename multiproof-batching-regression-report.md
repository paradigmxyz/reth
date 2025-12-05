# Multiproof Batching Regression Analysis Report

## Executive Summary

The multiproof batching PR introduces a **324% regression (4.25x slower)** in `newPayload` latency. Analysis of Jaeger traces and Prometheus metrics reveals **two interacting issues** that break pipeline parallelism.

### The Core Problem: Batching + Disabled Chunking

| Metric | Baseline | Feature | Change |
|--------|----------|---------|--------|
| **Total Duration** | 139ms | 590ms | **+324% (4.25x)** |
| Total Spans | 9,937 | 23,924 | +140% |
| `validate_block_with_state` | 122ms | 555ms | +355% |
| `payload processor` | 113ms | 309ms | +173% |
| `storage worker` | 121ms | 321ms | +165% |

### Root Cause (from Prometheus metrics)

1. **Batching creates large proof tasks**: p99 batch size = 16 messages (hitting limit)
2. **Chunking is almost never happening**: p99 chunks = 1 (should be 5-10)
3. **Large proofs take much longer**: p90 proof duration = 40ms vs p50 = 2.2ms (18x slower)
4. **Chunking disabled when workers busy**: `should_chunk` requires `available_workers > 1`

This creates a **vicious cycle**: large batch → workers busy → no chunking → single huge proof → workers wait → next large batch...

### Critical Observation

In the **baseline**, `payload processor` (113ms) and `storage worker` (121ms) **overlap in time** (run in parallel).
- Total time ≈ max(113ms, 121ms) = ~122ms ✓

In the **feature**, `payload processor` (309ms) and `storage worker` (321ms) run **sequentially**.
- Total time ≈ 309ms + 321ms - overlap = ~555ms ✗

**The batching serializes what should be parallel work because chunking is disabled.**

---

## System Architecture

The multiproof system is designed for **pipelined parallelism** across 3 concurrent threads:

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           EXPECTED TIMELINE (parallel)                       │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  Thread 1 (Main):        ┌──────────────────────────────────────┐           │
│  execute_block           │ TX0 → TX1 → TX2 → TX3 → ... → TXn   │           │
│                          └──┬────┬────┬────┬─────────────┬──────┘           │
│                             │    │    │    │             │                  │
│                             ▼    ▼    ▼    ▼             ▼                  │
│                          StateUpdate messages (1 per TX)                    │
│                             │    │    │    │             │                  │
│  Thread 2 (MultiProof):     ▼    ▼    ▼    ▼             ▼                  │
│  MultiProofTask::run     ┌──────────────────────────────────────┐           │
│                          │ Recv → Dispatch → Recv → Dispatch... │           │
│                          └──┬────────────┬────────────┬─────────┘           │
│                             │            │            │                     │
│                             ▼            ▼            ▼                     │
│  Thread 3 (Workers):     ┌─────┐      ┌─────┐      ┌─────┐                  │
│  Proof computation       │ P1  │      │ P2  │      │ P3  │                  │
│                          └─────┘      └─────┘      └─────┘                  │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Key insight**: The system achieves performance through overlap:
- While TX5 executes, proof for TX3 computes, proof for TX1 applies to trie
- Total time ≈ max(execution, proof_work), not sum

---

## Trace Evidence

### Baseline (139ms) - Proper Parallelism

![Baseline Trace](/.playwright-mcp/baseline-trace-overview.png)

**Timeline visualization shows overlapping spans:**
```
Time →   0ms          50ms         100ms        139ms
         │            │            │            │
payload  ████████████████████████████████████   (113.66ms)
processor│                                  │
         │                                  │
storage  │████████████████████████████████████  (121.71ms) ← OVERLAPS!
worker   │                                  │
         │                                  │
         └──────────────────────────────────┘
                    PARALLEL
```

```
on_new_payload: 122.42ms
└── validate_block_with_state: 122.31ms
    ├── spawn_payload_processor: 1.23ms
    ├── payload processor: 113.66ms ─────────┐
    │   └── (executes transactions)          │ PARALLEL
    ├── storage worker: 121.71ms ────────────│ (overlapped)
    │   └── (many small Storage proofs)      │
    └── await_state_root: ~8ms               ┘
```

**Key observations:**
- `payload processor` and `storage worker` **start at nearly the same time**
- They run **concurrently** - visible as overlapping bars in Jaeger timeline
- `get_overlay`: 10μs (cache hit)
- Total time = max(113ms, 121ms) ≈ 122ms ✓

### Feature (590ms) - Broken Parallelism

![Feature Trace](/.playwright-mcp/feature-trace-overview.png)

**Timeline visualization shows sequential spans:**
```
Time →   0ms         150ms        300ms        450ms        590ms
         │            │            │            │            │
payload  ████████████████████████████████████████            (309.26ms)
processor│                                    │
         │                                    │
storage  │                                    ████████████████████████████  (321.76ms)
worker   │                                                              │
         │                                                              │
         └──────────────────────────────────────────────────────────────┘
                              SEQUENTIAL!
```

```
on_new_payload: 555.39ms
└── validate_block_with_state: 555.21ms
    ├── spawn_payload_processor: 1.74ms
    ├── payload processor: 309.26ms ─────────┐
    │   └── (executes transactions)          │
    │                                        │ SEQUENTIAL!
    ├── storage worker #1: 321.76ms ─────────│ (starts AFTER processor!)
    │                                        │
    ├── storage worker #2: 320.88ms ─────────│
    └── await_state_root: 7.96ms             ┘
```

**Key observations:**
- `storage worker` spans **start AFTER** `payload processor` advances
- They are **not overlapping** - visible as sequential bars in Jaeger
- Total time ≈ 309ms + 321ms - small_overlap ≈ 555ms ✗
- The batching causes workers to wait for batched messages instead of processing incrementally

---

## Root Cause Analysis

### 1. The Primary Problem: Batching Breaks Pipeline Parallelism

The multiproof system is designed as a **3-stage pipeline**:
1. **Execution thread**: Executes transactions, sends `StateUpdate` messages
2. **MultiProof coordinator**: Receives messages, dispatches to workers
3. **Storage workers**: Compute proofs in parallel

**The key to performance**: All 3 stages should be active simultaneously, overlapping work.

### 2. How Batching Destroys the Pipeline

The batching code uses `try_recv()` to drain messages:

```rust
// multiproof.rs - StateUpdate handling
loop {
    if estimated_targets >= DEFAULT_MAX_BATCH_TARGETS ||  // 500
        num_batched >= DEFAULT_MAX_BATCH_MESSAGES          // 16
    {
        break;
    }
    match self.rx.try_recv() {  // <-- NON-BLOCKING DRAIN
        Ok(MultiProofMessage::StateUpdate(...)) => {
            merged_update.extend(next_update);
            num_batched += 1;
        }
        Err(_) => break,  // Channel empty
    }
}
// Process the accumulated batch
self.on_state_update(source, merged_update);  // <-- BLOCKS until complete
```

**The problem**: `try_recv()` is too fast - it drains messages faster than workers can process them:

**Without batching** (baseline) - Pipeline flows:
```
Time →
Execution:   TX0 ──── TX1 ──── TX2 ──── TX3 ──── TX4
              │        │        │        │        │
              ▼        ▼        ▼        ▼        ▼
MultiProof:  recv → dispatch → recv → dispatch → recv...
              │        │        │        │
Workers:     ████     ████     ████     ████    (small proofs, parallel)
             │        │        │        │
             ▼        ▼        ▼        ▼
Result: Execution and proof computation OVERLAP. Total time = max(exec, proof).
```

**With batching** (feature) - Pipeline stalls:
```
Time →
Execution:   TX0 ─ TX1 ─ TX2 ─ TX3 ─ TX4 ─ TX5 ─ TX6 ... ████ (blocked waiting)
              │    │    │    │    │    │    │
              ▼    ▼    ▼    ▼    ▼    ▼    ▼
             [all instantly drained by try_recv]
                                │
MultiProof:  ─────────────────────────────────────→ dispatch ONE batch
                                                        │
Workers:                                             ██████████████████████
                                                     (one HUGE proof)
                                                        │
                                                        ▼
Result: Execution completes, THEN proof runs. Total time = exec + proof.
```

### 3. Why This Happens: Message Accumulation

1. `try_recv()` is **instantaneous** - drains the channel in microseconds
2. Execution produces ~200 `StateUpdate` messages per block
3. With batching, these get merged into ~12-13 large batches
4. Each batch creates a **single large proof task**
5. While processing one large proof, execution backs up
6. When proof finishes, execution can proceed, creating more backed-up messages
7. The pipeline becomes **stop-and-go** instead of **continuous flow**

### 4. Secondary Issue: Cache Misses (Minor Contributor)

The overlay cache also shows degradation, but this is a **symptom** not the root cause:

| Span | Baseline | Feature |
|------|----------|---------|
| `get_overlay` | 10μs | 28.9ms |

This happens because:
- Batching changes proof computation timing
- Workers may query state at unexpected DB tips
- Cache misses trigger expensive trie revert lookups

However, even fixing this would only save ~30ms per miss. The **555ms→122ms gap** is primarily from lost parallelism.

---

## The Fundamental Tradeoff

| Batching Benefits (theoretical) | Batching Costs (actual) |
|--------------------------------|-------------------------|
| Deduplication of overlapping targets | Larger proofs = longer per-proof time |
| Reduced trie traversals | Fewer proofs = less worker parallelism |
| | Blocking on large batch = pipeline stalls |
| | Changed timing = cache misses |
| | Loss of execution/proof overlap |

**The regression shows costs > benefits** because:
1. Deduplication benefit is small (targets don't overlap much between consecutive TXs)
2. Parallelism cost is huge (proof computation doesn't scale linearly with targets)
3. Pipeline parallelism is the **primary** performance mechanism
4. Cache invalidation adds ~30ms per miss

---

## Quantified Impact

### From Dashboard Metrics

| Metric | Value |
|--------|-------|
| Raw StateUpdate messages per block | 195-207 |
| With batch limit of 16 messages | ~12-13 batches/block |
| Batch targets limit | 500 |

### Time Breakdown (Feature)

| Component | Time | % of Total |
|-----------|------|------------|
| `validate_block_with_state` | 555ms | 94% |
| `payload processor` | 309ms | 52% |
| `storage worker` (×2) | 321ms each | 54% |
| `get_overlay` (cache miss) | 29ms | 5% |
| `trie_reverts` retrieval | 22ms | 4% |

---

## Prometheus Metrics Analysis

### Batch Size Distribution (Feature Branch)

Data from `reth_tree_root_state_update_batch_size_histogram`:

| Percentile | Batch Size (messages) | Interpretation |
|------------|----------------------|----------------|
| p0 (min) | 1 | Some batches are single messages |
| **p50** | **1** | Half of batches are single messages |
| p90 | 5 | 10% of batches have 5+ messages |
| p95 | 8 | 5% of batches have 8+ messages |
| **p99** | **16** | 1% of batches hit the limit |
| **max** | **16** | Hitting `DEFAULT_MAX_BATCH_MESSAGES` limit |

**Key Finding**: While p50 batch size is 1 (good), the p99 and max are hitting the 16-message limit. These large batches are the problem.

### Chunking Behavior

Data from `reth_tree_root_state_update_proof_chunks_histogram`:

| Percentile | Chunks | Interpretation |
|------------|--------|----------------|
| p0-p99 | **1** | Almost all dispatches are NOT chunked |
| p99.9 | 3 | Very rare chunking |
| max | 11 | Maximum chunks observed |

**Critical Finding**: Chunking is almost never happening (p99 = 1 chunk). Large batches are being dispatched as single monolithic proof tasks instead of being split across workers.

### Why Chunking Doesn't Happen

The chunking decision in `on_prefetch_proof()` and `on_state_update()`:

```rust
// Line 785-787 in multiproof.rs
let should_chunk = self.multiproof_manager.proof_worker_handle.available_account_workers() > 1 ||
    self.multiproof_manager.proof_worker_handle.available_storage_workers() > 1;
```

**Chunking only happens if workers are available!** When workers are busy processing previous large batches, `should_chunk = false`, and the next large batch is dispatched as ONE task.

### Worker Utilization

Data from `reth_tree_root_active_*_workers_histogram`:

| Metric | p50 | p90 | max |
|--------|-----|-----|-----|
| Active Storage Workers | 5 | 32 | 32 |
| Active Account Workers | 9 | 32 | 32 |

Workers ARE being utilized (p90 = 32 = saturated). But they're doing **fewer, larger proofs** instead of **many small proofs**.

### Proof Calculation Duration

Data from `reth_tree_root_proof_calculation_duration_histogram`:

| Percentile | Duration | Interpretation |
|------------|----------|----------------|
| p50 | **2.2ms** | Median proof is fast |
| p90 | **40ms** | 10% of proofs take 40ms+ |
| p95 | 48ms | |
| p99 | **56ms** | 1% take 56ms+ |
| max | **79ms** | Worst case is 79ms |

**Key Finding**: The large batched proofs (p90+) take **18-36x longer** than median proofs. This is where parallelism is lost - one worker spends 40-80ms on a huge proof while others sit idle.

### The Vicious Cycle

The metrics reveal a **feedback loop**:

```
1. Large batch arrives (16 messages)
         ↓
2. Workers are busy (available_workers ≤ 1)
         ↓
3. should_chunk = false
         ↓
4. Entire batch dispatched as ONE proof task
         ↓
5. One worker gets huge task (40-80ms)
         ↓
6. Other workers finish, sit idle
         ↓
7. Next batch arrives, workers still busy with huge task
         ↓
8. REPEAT → serialized execution
```

---

## Potential Solutions

### Option 1: Remove Batching Entirely
Revert to baseline behavior. Accept that deduplication isn't worth the parallelism cost.
- **Pros**: Guaranteed to restore baseline performance
- **Cons**: Loses any potential deduplication benefits

### Option 2: Much Smaller Batch Limits
```rust
const DEFAULT_MAX_BATCH_TARGETS: usize = 50;   // was 500
const DEFAULT_MAX_BATCH_MESSAGES: usize = 2;   // was 16
```
- **Pros**: Simple change, some deduplication preserved
- **Cons**: May still break parallelism in some cases

### Option 3: Queue-Pressure Based Batching
Only batch when there's actual backlog:
```rust
let queue_len = self.rx.len();
if queue_len > BATCH_THRESHOLD {
    do_batching();  // Channel backing up - batch to catch up
} else {
    process_single_message();  // No backlog - process immediately
}
```
- **Pros**: Preserves parallelism normally, only batches when needed
- **Cons**: More complex, may have edge cases

### Option 4: Time-Bounded Batching
```rust
let batch_deadline = Instant::now() + Duration::from_micros(100);
while Instant::now() < batch_deadline {
    match self.rx.try_recv() { ... }
}
```
- **Pros**: Limits accumulation time
- **Cons**: Adds latency floor

### Option 5: Fix Overlay Cache
Investigate why batching causes cache misses and fix the cache key/invalidation strategy.
- **Pros**: May allow batching to work
- **Cons**: Complex, may be architectural issue

---

## Recommendation

Based on the Prometheus metrics analysis, the root cause is **the interaction between batching and chunking**:

1. Batching creates large proof tasks
2. Chunking should split them across workers
3. But chunking is disabled when workers are busy
4. This creates a vicious cycle of large, serialized proofs

### Recommended Fix Priority

1. **Fix the chunking condition** (Highest Priority)

   Change the `should_chunk` logic to always chunk large batches:
   ```rust
   // BEFORE (line 785-787):
   let should_chunk = self.multiproof_manager.proof_worker_handle.available_account_workers() > 1 ||
       self.multiproof_manager.proof_worker_handle.available_storage_workers() > 1;

   // AFTER: Always chunk if over threshold, regardless of worker availability
   let should_chunk = true;  // Or remove the condition entirely
   ```

   This ensures large batches get split across workers even when workers are busy.

2. **Reduce batch limits** (Secondary)
   ```rust
   const DEFAULT_MAX_BATCH_MESSAGES: usize = 4;   // was 16
   const DEFAULT_MAX_BATCH_TARGETS: usize = 100;  // was 500
   ```

   This reduces the severity of the problem even if chunking doesn't fully fix it.

3. **Add observability** (For validation)
   ```rust
   // Track when chunking is skipped
   pub chunking_skipped_due_to_busy_workers: Counter,
   pub available_workers_at_dispatch: Histogram,
   ```

### Expected Outcome

After fixing the chunking condition:
- Large batches will be split into 5-10 chunks instead of 1
- Multiple workers will process in parallel
- p90 proof duration should drop from 40ms to ~10ms
- Overall `newPayload` latency should approach baseline (~140ms)

---

## Files to Modify

- [`crates/engine/tree/src/tree/payload_processor/multiproof.rs`](crates/engine/tree/src/tree/payload_processor/multiproof.rs)
  - **Lines 33-40**: Batch size constants (`DEFAULT_MAX_BATCH_TARGETS`, `DEFAULT_MAX_BATCH_MESSAGES`)
  - **Lines 785-787**: `should_chunk` condition in `on_prefetch_proof()` - **PRIMARY FIX**
  - **Lines 899-901**: Similar `should_chunk` condition in `on_state_update()` - **PRIMARY FIX**
  - Lines 1048-1146: `process_multiproof_message()` batching logic

- [`crates/storage/provider/src/providers/state/overlay.rs`](crates/storage/provider/src/providers/state/overlay.rs)
  - Lines 313-352: `get_overlay()` and cache logic
  - Lines 203-308: `calculate_overlay()` - understand why cache misses

---

## Appendix: Trace Screenshots

### Baseline Trace
- URL: http://localhost:16688/trace/7db927776df848bd801482b0eb77f49e
- Service: reth-baseline
- Duration: 139.54ms
- Screenshot: [baseline-trace-overview.png](/.playwright-mcp/baseline-trace-overview.png)

### Feature Trace
- URL: http://localhost:16688/trace/57b37ec9fbd831a035ade2833e1bb7cd
- Service: reth-feature
- Duration: 590.8ms
- Screenshot: [feature-trace-overview.png](/.playwright-mcp/feature-trace-overview.png)
