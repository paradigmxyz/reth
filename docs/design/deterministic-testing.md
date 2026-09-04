# Deterministic simulation testing

Reth uses Commonware's seeded executor to schedule cooperative tasks, advance virtual time, and
replay application traces. The node integration assembles real transaction admission, payload
building, EVM validation, forkchoice, live block download, and persistence under this executor.
Production and simulation share their application drivers. Separate Loom and real-database crash
tests cover properties that the cooperative executor cannot establish.

## Running and replaying

Run the simulation campaigns with:

```sh
cargo nextest run -p reth-tasks -p reth-engine-tree -p reth-static-file-types -p reth-provider \
  -p reth-basic-payload-builder -p reth-eth-wire -p reth-trie-parallel -p reth-execution-cache \
  --lib -E 'test(deterministic_)'
```

The engine's development dependencies enable `reth-tasks/deterministic`. When testing the task
crate alone, pass `--features deterministic` explicitly. The default campaigns run seeds 0 through
15. To reproduce or explore a particular seed:

```sh
RETH_DST_SEED=42 cargo nextest run \
  -p reth-tasks -p reth-engine-tree -p reth-static-file-types -p reth-provider \
  -p reth-basic-payload-builder -p reth-eth-wire -p reth-trie-parallel -p reth-execution-cache \
  --lib -E 'test(deterministic_)'
```

Each simulated scenario runs twice and compares Commonware's runtime audit digest plus application
results or traces. Cache tests compare repeated constructions and collision behavior directly.
Fixtures use explicit inputs. Replay requires the same source, lockfile, scenario,
and seed; a dependency upgrade can change schedules. Commonware is pinned to `2026.7.1` and is
optional in `reth-tasks`, enabled by the `deterministic` feature.

Run the additional concurrency and crash checks with:

```sh
cargo nextest run -p reth-engine-tree -p reth-provider --lib \
  -E 'test(loom_) | test(engine_step_) | test(real_storage_commit_crash_recovery)'
```

## Task execution, time, and channels

`reth_tasks::TaskRuntime` supplies execution capabilities without exposing the underlying executor.
Construct it from a production `Runtime`, or use `TaskRuntime::deterministic(context)` inside a
Commonware runner. The caller must keep every simulated handle within that runner's lifetime.

| Operation | Production | Simulation |
| --- | --- | --- |
| `spawn` | Tokio task | Commonware future |
| `spawn_cpu` | Rayon pool, or Tokio blocking pool without the `rayon` feature | One atomic, schedulable closure |
| `spawn_blocking` | Tokio blocking pool | One atomic, schedulable closure |
| `spawn_named` | Persistent named OS worker | FIFO queue for that name |
| `spawn_named_or_blocking` | Named worker if available; otherwise Tokio blocking pool | Independent atomic closure, without waiting for the named lane |
| `spawn_dedicated` | New OS thread | One atomic, schedulable closure |
| `spawn_named_task` | Cooperative future on a persistent named OS worker | Cooperative future occupying the same FIFO lane until completion |
| `spawn_dedicated_task` | Cooperative future on its own OS thread | Commonware future |
| `now`, `sleep` | System time and Tokio timer | Commonware virtual clock |
| `yield_now` | Explicit future suspension | Explicit scheduling point |
| `bounded_channel` | Tokio MPSC | The same asynchronous channel, polled by Commonware |

Synchronous jobs must be bounded and must not block on another simulated actor. For nested CPU
work, return the child handle from the closure and await it from a cooperative caller. A named
future owns its lane even while awaiting input; it cannot await a later job on that same lane.
Production and simulation share these queue semantics, including mixed synchronous and
asynchronous jobs. `spawn_named_or_blocking` explicitly permits independent execution while a
named worker is occupied, as required for competing payload-build attempts.

`TaskHandle` reports completion, panic, cancellation, or executor shutdown. Dropping a handle
detaches its task; `abort_on_drop()` opts into cancellation. `abort()` stops a future at a poll
boundary or prevents a queued closure from starting. An already-running synchronous closure may
finish and produce effects even after its handle reports cancellation. A canceled handle is not
a barrier proving that the task's resources have been dropped.

Async Tokio MPSC channels and oneshots can be polled without Tokio's scheduler. Use `send().await`
and `recv().await` for simulated actors. This is not a guarantee for every channel implementation:
Tokio watch notifications select internal buckets using ambient randomness, which can change
wake order. The simulated transport therefore uses shared state and a single `Notify` for its
control signals. Tokio timers, task spawning, and socket I/O still need an explicit runtime
boundary. Blocking Crossbeam or standard-library receives cannot run on the
simulator's single thread while waiting for another actor. Existing channel types need not all
be replaced: the engine uses nonblocking selection over its production Crossbeam channels.

`deterministic_tasks` replays the same workload exercised by `production_tasks`: shared tasks,
CPU and blocking jobs, named and dedicated workers, nested jobs, queue ordering, bounded-channel
backpressure, clock advancement, panic, cancellation, and detachment. Production-only assertions
also check OS-thread identity. Simulation does not reproduce worker thread-local state, Rayon
work stealing, or preemption inside a closure.

## Shared application drivers

The receipt worker consumes an asynchronous stream on a persistent named production thread.
Simulation runs the same `ReceiptRootTaskHandle::run` future. Each poll encodes at most 64 receipts
before yielding, including pending receipts unblocked by a late low-index receipt. Dropping the
result receiver stops computation and closes the input once observed. Production uses an
unbounded Tokio input channel so synchronous execution and Rayon producers can send without
blocking; its input count is bounded by the block's transactions. Simulation uses a bounded
channel to exercise backpressure and producer wakeup during cancellation.

The engine's blocking `run()` and simulated driver share the same `step_async()` function. The
native `step()` wrapper drives that future synchronously and can wait for an event; simulation
polls without blocking and yields between steps and during cooperative validation. The
shared selector preserves persistence-first priority and persistence backpressure. Termination
is incremental: it stops consuming engine messages and drains outstanding writes. Successful
termination flushes the remaining block and state/trie frontiers, including state-only catchup
when masking has kept state persistence behind block persistence. The completion signal is also
released on persistence failure or handler drop; it does not alone certify durability.

| Campaign | Scheduled operations | Correctness checks |
| --- | --- | --- |
| `deterministic_receipt_streams` | Four producers send indexed receipts; the production driver consumes them, is aborted, or loses its result receiver. | Root and bloom match Alloy's sequential calculation across out-of-order delivery and the RLP index-128 boundary. Gaps and known-length truncation suppress results. Cancellation closes input and wakes blocked producers. |
| `deterministic_persistence_handoff` | Forkchoice updates, payload-build lease release, automatically requested persistence completions, queued engine input, and shutdown. | Payload-build lease notifications share the engine event loop; block and state/trie frontiers advance independently; shutdown catches state up without consuming queued input. |
| `deterministic_snapshots_across_persistence` | Reader snapshots and reads interleave with real commits and separate memory eviction. | Old and new snapshots return expected hashes, transactions, and receipts without gaps across the disk/memory boundary. A hook also forces commit and eviction during snapshot construction. |
| `deterministic_persistence_worker_actions` | Save, remove, safe/finalized updates, queue drain, and worker restart through the production persistence action handler. | Real database frontiers, acknowledgments, metrics, and error propagation agree with the native worker workload. |
| `deterministic_payload_job_clock_and_cancellation` | Payload-service deadlines, build intervals, permit waits, cancellation, and detached fallback builds. | Virtual deadlines and burst ticks preserve timer semantics; job removal cancels work and detached attempts retain their resource leases until completion. |

The handoff campaign supplies known executed blocks and controlled persistence acknowledgments.
The node campaign below executes payloads and runs the real persistence worker. Real elapsed time
used only for metrics is excluded from replay traces.

## Node integration

`deterministic_node_executes_downloads_persists_and_restarts` runs two independent datadirs on a
Cancun-active test chain. Each node has its own named-worker queues under the same Commonware
executor. It repeats this transfer scenario for every seed:

1. Admit five signed transactions per block into the real Ethereum pool. Let the persistent
   prewarming worker publish an immutable snapshot and verify its reads match the parent state.
   Submit forkchoice with payload attributes to the engine, and resolve jobs through
   `PayloadBuilderService` and `BasicPayloadJobGenerator` using the Ethereum payload builder.
2. Execute and validate the resulting payloads through `newPayload`, then change the canonical
   chain through forkchoice. Build competing branches and reorg before extending the winner.
   Rehashed headers with incorrect receipt and state roots must fail validation.
3. Give the follower an unknown head. Its `EngineHandler` and `BasicBlockDownloader` fetch headers
   and bodies from a test peer over ETH messages, execute the downloaded blocks, and converge on
   the producer's canonical hashes and account state.
4. Observe threshold-triggered writes, gracefully stop both engines, and drain persistence.
   Release the follower's database handles and memory overlays, reopen the datadir, run production
   consistency checks, execute another batch, and reopen again to check its persisted state.

The test compares canonical hashes, account nonce and balance, actual speculative transaction
counts, protocol events, and the Commonware audit digest. Every node, including the restarted one,
must execute speculative transactions. It never inserts pre-executed blocks into either engine.
Database ownership is checked before reopening, so restart cannot accidentally reuse a live MDBX
environment.
The scenario runs on an explicitly sized thread stack to accommodate synchronous EVM validation.
The node and persistence-worker campaigns exercise independent block/state persistence
frontiers, which are enabled unconditionally.

```sh
cargo nextest run -p reth-engine-tree --lib \
  -E 'test(deterministic_node_) | test(persistence_worker_actions)'
```

`EngineApiTreeHandler::new_from_provider` constructs the same handler used by the native launcher.
Its `run_cooperative(TaskRuntime)` driver awaits the shared step function and yields between
events. An explicit `reth_newPayload` persistence wait is deferred without blocking the executor;
later input stays queued until that write completes. Payload-build completion notifications keep
their normal priority; overlay providers retain snapshots of their parent blocks across persistence.

The node uses `BasicEngineValidator::with_cooperative_sparse_trie(TaskRuntime)` followed by
`with_cooperative_prewarming()` and `with_txpool_prewarming(source)`. Authoritative EVM execution,
transaction conversion, and receipt accumulation remain synchronous. The real sparse-trie hashing,
proof, and update workers run as separate cooperative tasks. Validation awaits their results, so
the scheduler can advance other node services while a root is pending. The engine's native and
cooperative drivers share this validation path; the native entry points drive it synchronously.

Sparse-trie updates, proof revelation, root calculation, retained nodes, and epoch pruning use the
production algorithms. Account proofs await their storage-proof dependencies before entering the
synchronous account encoder. Two account workers and two storage workers allow independent proofs
to finish in different orders. Each proof calculation opens and drops its database cursors within a
bounded operation. An explicit per-trie setting replaces internal Rayon dispatch with sequential
processing, and task dispatch sorts targets where their order is otherwise unspecified. Scheduling
points exist between worker operations; they do not interrupt individual trie walks or EVM opcodes.

The cooperative profile preserves fully finalized sparse tries before publishing root results,
so the next block can reuse a trie without a blocking wait on another simulated actor. Shutdown
drains hashing and proof workers, including pipelines canceled by invalid payloads, before closing
the databases. Sparse worker failures and timeouts are surfaced instead of triggering the native
serial fallback; otherwise a fallback could hide a sparse-trie bug from the simulation. The node
asserts that validated roots have corresponding preserved sparse tries.

`with_sequential_execution()` remains available as a synchronous reference profile with prewarming
disabled. Both profiles disable shared payload-builder execution/trie resources, JIT support, and
BAL parallel execution. Payload building still computes its own synchronous root; this scenario
does not yet share the engine's sparse-trie pipeline with the builder.

### Prewarming

Block prewarming runs the production speculative EVM transaction body against the shared execution
cache and emits real sparse-trie access hints. Two scheduled transaction jobs can complete in
different orders. Each job opens and drops its provider and EVM within the operation, before the
driver yields. Validation gives these workers a one-millisecond virtual window before opening
the authoritative state provider. The node runner advances ten microseconds per scheduling cycle
so multiple worker polls fit in that window. The authoritative block itself remains atomic: this
profile does not interleave prewarming with individual authoritative transactions or opcodes. Focused
worker tests separately advance the executed-transaction index between jobs and exercise stopping
speculative work.

The cache coordinator waits for speculative jobs to finish, then applies the authoritative bundle
and publishes a cache for a valid block. Cooperative waits for block validity happen outside the
cache mutex. Invalid or abandoned validation cannot publish speculative post-state as canonical
state. Engine shutdown drains the cache actors before releasing the databases.

Execution caches use the real bounded cache, collision eviction, and epoch invalidation logic.
`ExecutionCache::new_deterministic` fixes the hash seeds and sorts bundle insertion, since ambient
hash seeds and randomized map iteration would otherwise change eviction winners across replays.
The native constructor retains its normal hashing and insertion policy.

The persistent txpool worker consumes a live best-transactions iterator for the canonical parent.
It executes one speculative transaction per cooperative CPU job and uses virtual time for idle
polling and snapshot publication. The native worker retains its wall-clock batch budget. Shared
command handling preserves nested pauses, resumption, and head replacement; a new parent resets
the mutable read cache. Published snapshots remain immutable and are only used for their matching
parent. Validation and payload-build leases pause this worker, and shutdown waits for its active
job to release its provider. The node harness advances the pool's tracked head directly because
the pool maintenance actor is still outside the scenario.

The cooperative BAL prewarmer also schedules actual account/storage prefetches and hashed-state
streaming through the runtime. Authoritative BAL updates finish even when speculative prefetching
is stopped. Focused tests cover this path; the node scenario remains on a Cancun-active chain
with transaction-based prewarming and synchronous authoritative execution.

`deterministic_block_prewarm_cache_lifecycle`, `deterministic_bal_prewarm_reads_and_streams_state`,
and `deterministic_txpool_prewarming` check actual account, contract bytecode, and storage reads.
They exercise cancellation, invalid-block cache handling, parent changes, and immutable snapshot
publication. Separate checks compare the native and cooperative pool workers and verify that a
canceled shutdown wait can be retried without losing its resource-lifetime barrier.

The campaign also validates a built block outside Commonware using both the native parallel
configuration and the sequential profile, comparing recovered transactions, EVM output, hashed
state, and the database contents after applying trie updates. The synchronous strategy can
re-emit unchanged nodes that the sparse strategy omits, so their raw patches need not match.

`deterministic_sparse_hashing_proofs_and_cancellation` separately drives multiple account and
storage changes through the cooperative worker, compares with native sparse computation, and
exercises delayed stream completion, cancellation, and incomplete update streams. Sparse-trie
tests also compare sequential dispatch against parallel dispatch across updates and pruning.
`deterministic_proof_workers` compares cooperative account/storage proofs with an independent
synchronous proof oracle, including storage-only requests, dropped consumers, provider errors,
and shutdown with queued work.

`BasicPayloadJobGenerator::with_task_runtime` routes job clocks and all build attempts through the
injected runtime. Native constructors retain Tokio timers. A synchronous builder attempt must not
wait for another simulated actor or start uncontrolled work internally. The node profile uses
the Ethereum builder without shared engine caches or background state-root resources.

`PersistenceHandle::spawn_service_with_runtime` runs the production action handler as a cooperative
worker, yielding between actions and between save acknowledgment and pruning. Drop every sender
and await its handle to drain the queue. Aborting the worker can discard queued actions. Native
database calls remain atomic from the simulator's perspective; in particular, an unwind must not
wait for a database reader held by another suspended simulated actor.

The profile assembles node components directly. The stock node launcher, JSON-RPC sockets,
discovery and session management, historical backfill pipeline, and transaction-pool maintenance
actor are not part of this scenario. The harness updates the pool after resolving a built payload
and provides a peer adapter backed by the producer's accepted blocks. These boundaries must be
converted before the stock node can run unchanged under simulation.
Adding other transaction types also requires auditing their inputs: for example, blob admission
near Osaka currently reads wall time. Passing a seeded runtime does not redirect that read.

## Network transport and protocol time

`reth_eth_wire::simulation` is available with `test-utils`. `SimulatedLink` connects bounded
in-memory byte streams through two scheduled forwarding actors. A seed controls fragmentation
and per-fragment latency/jitter. The test can partition, resume, or disconnect the link. A
partition pauses future forwarding; already delivered bytes remain readable, and a fragment
already in flight may arrive. Each direction preserves TCP byte ordering.

`authenticated_pair` uses those byte streams for real ECIES authentication, encrypted framing,
RLPx Hello/capability negotiation, compression, and P2P messages. Simulation-only ECIES constructors
take seeded entropy for nonces, ephemeral keys, encryption keys, IVs, and authentication padding.
Ordinary constructors retain production randomness. P2P handshake deadlines, ping/pong timers,
and ping rate limiting use the injected clock; ordinary streams retain native timers and time.
The node peer then performs the regular ETH Status handshake and exchanges encoded ETH requests
and responses over the authenticated stream.

The transport campaigns exercise fragmented messages, bounded-buffer backpressure, partition
recovery, disconnects, malformed ETH data, encrypted-frame corruption, idle ping/pong exchanges,
and ping timeout under a partition. They compare hashes of the actual forwarded ciphertext as
well as runtime audits. The node's peer adapter correlates request IDs and
permits responses to overtake one another; dropping or timing out a request removes its pending
entry so a late response cannot satisfy another request. Separate lifecycle checks cover these
cases before the adapter is used by the real block downloader.

This simulates the ordered byte-stream boundary. It does not model TCP packet retransmission,
kernel socket buffers, DNS, discovery, or the node's session manager. Fixed test identities and
seeded ECIES entropy are exclusively for test connections.

## Memory ordering

Payload-build leases share a small `PayloadBuildCounter` implementation between production
atomics and Loom atomics. Loom checks concurrent final releases, visibility of completed-job
writes after observing an inactive counter, and reacquisition after a stale completion hint.
The production engine serializes lease acquisition with persistence decisions; releases may
happen concurrently. A completion notification is only a hint, so the engine rechecks the
counter before deciding which persistence work can start.

These tests explore the counter's memory ordering. They do not model Crossbeam internals or all
shared state in the engine, which remain covered by their own tests and libraries.

## Storage faults and real-backend recovery

The changeset-offset sidecar uses a private file-operation boundary shared by its production
writer and a simulated file. The production file still uses the existing 16-byte records. The
simulator tracks live and durable bytes, invalidates handles on crash, and injects partial-write
errors, sync errors, resize errors, and loss or partial retention of unsynced data. Recovery uses the
same append, truncate, read, and sync logic as production. After an I/O error the writer must be
discarded and reopened, as its in-memory offset can no longer be trusted.

`deterministic_sidecar_fault_recovery` checks that recovery preserves the committed prefix,
discards uncommitted tails, rejects a file shorter than its committed length, and persists its
repair across another crash. It also exercises unsynced truncation and errors both before and
after a sync or resize takes effect. Header publication is modeled atomically. Other NippyJar
files and arbitrary corruption of complete records are outside this model; the sidecar has no
per-record checksum.

MDBX and RocksDB perform their own I/O and cannot be virtualized by replacing a Rust file trait.
The snapshot campaign therefore schedules around synchronous real-provider calls, with each
call completing atomically from the simulator's perspective.

`real_storage_commit_crash_recovery` separately exercises actual MDBX, RocksDB, and static files.
A child process commits an acknowledged baseline, starts the next commit, and exits without
Rust destructors after static-file finalization, after each of three RocksDB batches, or after
MDBX commit. The parent reopens the data, runs production consistency repair, checks the retained
prefix across headers, receipts, lookup tables, account state, and history, reopens again to
verify repair persists across reopening, and resumes persistence. The ignored `commit_crash_child`
test is invoked only by this parent test with an explicit temporary directory and cut point.

These are process-crash tests: the host kernel and page cache remain alive. They do not establish
power-loss safety or explore cuts inside backend FFI. Simulating those requires a lower I/O
boundary or a separate filesystem/VM fault harness, validated against the real storage engines.

## Converting another subsystem

Pass `TaskRuntime` into the component, use its clock and spawn operations, and make long-running
workers asynchronous or expose nonblocking steps shared with production. Bound CPU work between
yields. Keep randomness and inputs seeded, avoid semantic dependence on hash-map iteration, and
compare application invariants as well as runtime audit digests. Add fault boundaries at the
storage operations the component owns, and retain real-backend tests to check the model.

The converted paths are deterministic at these explicit boundaries. Arbitrary raw Tokio/Rayon/
OS-thread call sites, database background threads, and the remaining node services are not under
the simulator's control. Introducing the facade does not intercept existing calls automatically.
