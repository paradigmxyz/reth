# Multiproof Diagnostic Logs Guide

This document describes the diagnostic logs added to trace the multiproof lifecycle and diagnose why `inflight_multiproofs_histogram` drops to 0.

## Branch
`yk/debug-multiproof`

## Log Locations and What They Tell You

### 1. **Initialization** (target: `tree::payload_processor`)
```
INFO ... storage_workers=X account_workers=Y max_proof_concurrency=Z max_multiproof_concurrency=W "Initialized proof worker system"
```
**Location**: `crates/engine/tree/src/tree/payload_processor/mod.rs:216`

**What it tells you**:
- How many worker threads were spawned
- The concurrency limits being used
- If `max_multiproof_concurrency` is 0 or too low, that's your problem!

**Expected values** (if config is 32):
- `max_proof_concurrency=32`
- `max_multiproof_concurrency=16` (half of proof concurrency)
- `storage_workers=32`, `account_workers=32` (minimum)

---

### 2. **Queue Full** (target: `tree::multiproof`)
```
INFO ... inflight=X max_concurrent=Y pending=Z "DEBUG: Queue is full, enqueuing multiproof request"
```
**Location**: `crates/engine/tree/src/tree/payload_processor/multiproof.rs:406`

**What it tells you**:
- The queue hit capacity (`inflight >= max_concurrent`)
- New requests are being queued instead of spawned
- If you see this immediately at startup with `inflight=0`, `max_concurrent` is probably 0

**Healthy state**: Should see this occasionally when busy, but not constantly

---

### 3. **Task Spawned** (target: `tree::multiproof`)

**Storage Proof**:
```
INFO ... inflight=X max_concurrent=Y "DEBUG: Spawned storage proof task"
```
**Location**: `multiproof.rs:531`

**Account Multiproof**:
```
INFO ... inflight=X max_concurrent=Y account_targets=A storage_targets=S "DEBUG: Spawned multiproof task"
```
**Location**: `multiproof.rs:628`

**What it tells you**:
- A task was successfully spawned
- The `inflight` counter incremented
- How many accounts and storage slots are being processed

**Expected**: Should see `inflight` incrementing from 0 up to `max_concurrent`

---

### 4. **Proof Calculated** (target: `engine::root`)
```
INFO ... sequence=X total_proofs=Y elapsed_ms=Z "DEBUG: Received ProofCalculated message, calling on_calculation_complete"
```
**Location**: `multiproof.rs:1131`

**What it tells you**:
- A worker completed its job and sent back a result
- The message reached the MultiProofTask event loop
- `on_calculation_complete()` is about to be called

**Key diagnostic**: If you see spawned tasks but NEVER see this log, workers are stalled or not completing!

---

### 5. **Task Completed** (target: `tree::multiproof`)
```
INFO ... prev_inflight=X new_inflight=Y pending=Z "DEBUG: Multiproof calculation completed"
```
**Location**: `multiproof.rs:426`

**What it tells you**:
- `on_calculation_complete()` was called
- The `inflight` counter decremented
- How many tasks are still pending

**Expected**: Should see `prev_inflight` decrement by 1 to `new_inflight`

**If you see**: `prev_inflight=0 new_inflight=0` repeatedly → the counter is already at 0 (broken state)

---

### 6. **Pending Task Dequeued** (target: `tree::multiproof`)
```
INFO ... pending_after_pop=X "DEBUG: Spawning queued multiproof from pending"
```
**Location**: `multiproof.rs:436`

**What it tells you**:
- After a task completed, a queued task was dequeued and spawned
- The pending queue is draining

---

## Diagnostic Scenarios

### Scenario A: Inflight stays at 0
**Symptoms**:
- No "Spawned" logs
- Only "Queue is full" logs with `inflight=0`

**Root cause**: `max_multiproof_concurrency` is 0 or too low
- Check log #1 for the initialization values

---

### Scenario B: Inflight increments but never decrements
**Symptoms**:
- See "Spawned" logs with inflight going 1, 2, 3...
- Never see "Received ProofCalculated" logs
- Never see "Task completed" logs

**Root cause**: Worker pool is stalled or not processing tasks
- Workers aren't completing
- `receiver.recv()` in spawn_blocking is blocking forever

---

### Scenario C: ProofCalculated received but inflight doesn't decrement
**Symptoms**:
- See "Received ProofCalculated" logs
- DON'T see corresponding "Task completed" logs

**Root cause**: `on_calculation_complete()` isn't being called (logic error)
- Check if the log at line 1131 is actually followed by the call at line 1139

---

### Scenario D: Healthy operation
**Expected log pattern**:
1. Initialization shows reasonable values
2. Spawned logs show inflight increasing (0→1→2→...→16)
3. ProofCalculated logs start appearing
4. Task completed logs show inflight decreasing
5. Pending tasks get dequeued when space opens up
6. Inflight oscillates between 0 and max_concurrent

---

## How to Use

1. Deploy this branch to your environment
2. Watch logs filtered by target:
   ```bash
   # All multiproof logs
   grep "tree::multiproof\|tree::payload_processor\|engine::root.*DEBUG:"
   
   # Just lifecycle events
   grep "DEBUG:"
   ```
3. Compare your logs against the scenarios above
4. Report back with:
   - The initialization log values
   - Whether you see spawned tasks
   - Whether you see ProofCalculated messages
   - Whether you see completion logs
   - The pattern of inflight values over time

---

## Quick Grep Commands

```bash
# See initialization
grep "Initialized proof worker system"

# Count spawned tasks
grep "DEBUG: Spawned" | wc -l

# Count completed tasks
grep "DEBUG: Multiproof calculation completed" | wc -l

# Count ProofCalculated received
grep "DEBUG: Received ProofCalculated" | wc -l

# Track inflight over time
grep "inflight=" | grep "DEBUG:" | awk '{print $NF}' | grep -oP 'inflight=\K\d+'
```

