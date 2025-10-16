# Proof Worker Context Refresh – Minimal Plan

## Goal
Stop respawning proof workers every block while still giving them the latest `ProofTaskCtx` and read-only transaction.

## Core Approach
1. Spawn the storage/account worker threads once during startup and keep the crossbeam queues alive.
2. Teach the worker job enums a single control message (`UpdateWorkerContext`) that carries a fresh read-only transaction and a refreshed `ProofTaskCtx`.
3. Before processing block *N+1*, wait until all block *N* jobs finish, broadcast one `UpdateWorkerContext` per worker, wait for acknowledgements, then resume normal job dispatching.

## Step-by-step Implementation Plan
1. Extend `ProofTaskCtx`
   - Add a `consistent_view: ConsistentDbView<Factory>` field.
   - Update the constructor, tests, and every call site (payload processor, multiproof task, benches) to pass the view along with the existing trie overlays.

2. Add control variants to worker jobs
   - Update `StorageWorkerJob` / `AccountWorkerJob` with `UpdateWorkerContext { tx, task_ctx, ack }` and `Shutdown`.
   - Keep the enums generic so the control message can carry the concrete transaction type.

3. Handle context refresh inside workers
   - Make `ProofTaskTx` mutable inside each worker loop.
   - On `UpdateWorkerContext`, replace the stored tx and `ProofTaskCtx`, rebuild cursor factories and any cached providers, then `ack`.
   - On `Shutdown`, break the loop so the thread exits cleanly.

4. Track in-flight jobs
   - Introduce an `Arc<AtomicUsize>` shared by handles and workers.
   - Increment when queuing any job (storage/account/blinded); wrap execution in an RAII guard that decrements on completion/panic.
   - Expose a helper to read the current count.

5. Wire into `PayloadProcessor`
   - Initialise the workers once during setup.
   - Before each new block: wait until `in_flight == 0`, send one `UpdateWorkerContext` (with a fresh tx from the ctx’s view) to each worker, await all acknowledgements, then hand out a refreshed handle for the next block.
   - Drop any old handles during the refresh window so callers cannot enqueue work mid-swap.

6. Shutdown path
   - When the owner drops the pool/handle, broadcast `Shutdown` once per worker so every thread exits.

## Integration Notes
- `PayloadProcessor` drives the refresh inline: wait for `in_flight == 0`, dispatch `UpdateWorkerContext` to each worker, await all acknowledgements, then resume normal job flow.
- Drop old handles before the refresh window so no new jobs sneak in while the workers are swapping state.
- On shutdown, send `Shutdown` once per worker so threads exit cleanly.

## Testing Checklist
- Unit test that `UpdateWorkerContext` swaps the worker’s tx/ctx and triggers the ack.
- Unit test that the in-flight counter blocks the refresh loop until outstanding work completes.
- Integration test that reuses the same workers across two block contexts and observes updated state.
