# Deterministic system testing for Reth

Status: implementation direction
Date: 2026-09-16

## Decision

Build the first useful system tester as an application-level deterministic simulator on top of
[PR #27017](https://github.com/paradigmxyz/reth/pull/27017). The simulator controls the choices
that change Reth's logical execution: workload generation, virtual time, task polling, Engine and
P2P message delivery, node lifecycle, and modeled storage results. It runs the production engine,
EVM, trie, proof, payload, download, and persistence components wherever #27017 already makes that
possible.

The durable failure artifact is a versioned semantic decision trace. A seed may explore a new run,
but a seed and Commonware audit digest are not sufficient reproducers. Replay checks at every step
that the recorded decision point and chosen option still exist. This both explains a failure and
detects replay drift after code changes.

Do not make VM execution, Linux thread control, coverage-guided scheduling, or snapshotting native
Rust futures prerequisites for the first campaign. Use three later validation lanes:

1. Run the same external workload trace and oracles on native multicore Reth to cover Tokio/Rayon
   preemption, weak memory, allocators, and performance feature selection.
2. Keep [PR #27058](https://github.com/paradigmxyz/reth/pull/27058) as evidence that Hermit can
   replay a narrow native Reth execution. Use it for minimized native-runtime investigations when
   its syscall model supports the case.
3. Qualify [Bedrock](https://github.com/oss-garage/bedrock) after the application simulator finds
   bugs. It can snapshot a whole Linux guest and control native thread scheduling, time, and
   randomness. It is the path for full-node runtime and filesystem behavior that cannot be
   represented faithfully at application boundaries.

Reth is an execution client, not a consensus implementation. The application simulator can test
Engine API sequencing, fork choice supplied by a simulated consensus layer, execution, trie and
persistence correctness, and EL P2P sync/gossip. Claims such as quorum safety or validator liveness
require a consensus client or a separate consensus model in the simulation.

## What #27017 establishes

#27017 is substantially more than a test executor. Its design and source establish these reusable
pieces:

- `reth_tasks::TaskRuntime` maps production work to Tokio, Rayon, named threads, and dedicated
  threads, and maps simulated work to Commonware cooperative futures or bounded atomic closures.
- Commonware supplies seeded task ordering, virtual time, an audit digest, in-memory storage,
  deterministic networking primitives, and a restart checkpoint.
- the engine's blocking and cooperative drivers share `step_async()`;
- the node integration runs real transaction admission, payload construction, EVM validation,
  fork choice, block download, persistence, database reopen, and another execution after restart;
- the real sparse-trie hashing, proof, update, retention, and pruning algorithms run with two
  cooperative account workers and two cooperative storage workers; and
- serial/native differential checks already compare roots, EVM output, hashed state, and persisted
  database contents.

Its present campaign is still a fixed scenario. It produces five transfers per block, a prescribed
fork and reorg, a prescribed follower download, and a prescribed restart. Commonware shuffles each
ready-task batch from one shared RNG. The same RNG also backs runtime randomness and Commonware
storage faults. Replay means rerunning the same seed and comparing an opaque audit digest.

The current Commonware API accepts a caller-provided `RngCore`, but it does not expose the ready
task set or a semantic scheduling callback. Its `Checkpoint` preserves virtual time, RNG, DNS,
audit state, and synchronized Commonware storage. It explicitly does not preserve pending tasks,
unsynchronized storage, network connections, or shutdown state. Reth's node scenario also uses
real MDBX, not Commonware storage.

The immediate work is therefore a decision-controller layer and generated actions around #27017,
plus a small Commonware scheduler hook. It is not a new executor.

## Current implementation

The first record/replay slice lives in `testing/dst`. It defines parsed semantic decision points,
counter-keyed exploration, a bounded binary trace, strict replay, and a retained recorder that can
be read after a panic. `reth-dst-trace` prints the binary artifact as a timeline. The engine-tree
node campaign builds its accepted block DAG online. From the current model state it can request
sibling payloads, import, mutate, or replay payloads, move fork choice among accepted blocks, and
sync a follower through the production ETH codec. Follower sync is split into begin and poll steps,
so the controller can advance virtual time, partition or heal the link, corrupt the next encrypted
response, or crash and restart the follower while requests are in flight.

Accepted blocks contain 5–64 signed transactions built with txgen-core's account, nonce, and
generation context APIs. The workload maintains nonces for 20 funded accounts and mixes legacy,
EIP-2930, and EIP-1559 envelopes, transfers, contract creation, and calls across four storage
contracts with abi-fuzz-generated keys and values. Transaction count is a semantic trace decision.
Persistence threshold, state-masking window, and multiproof chunk size vary over their legal
ranges by campaign seed and remain fixed across a node restart. Recovery uses the database tip
observed at the crash boundary rather than assuming which blocks were durable.

The bounded developer profile runs four cases:

```console
cargo run -p reth-dst-runner --features dst --bin reth-dst-node
```

This binary is the continuous campaign runner. A campaign can use a host deadline or a case count;
`RETH_DST_STEPS` bounds workload actions and `RETH_DST_MAX_DECISIONS` bounds semantic scheduling,
network, and storage choices per case. Product panics are caught per case, printed as `BUG` lines,
and written under `target/reth-dst/failures`. Cases that exhaust the decision bound print
`INCONCLUSIVE`, write a trace under `target/reth-dst/inconclusive`, and let the campaign continue.
`RETH_DST_ARTIFACT_DIR` replaces `target/reth-dst` as the artifact root.
Successful cases are not replayed during a campaign. Set `RETH_DST_VERIFY_PASSES=1` to replay each
successful trace when validating simulator determinism; failures are always retained for strict
replay with `RETH_DST_REPLAY`.

```console
RETH_DST_SECONDS=3600 RETH_DST_STEPS=1000 \
  cargo run -p reth-dst-runner --release --features dst --bin reth-dst-node

cargo run -p reth-dst --bin reth-dst-trace -- target/reth-dst/failures/node-7-12345678.dst

RETH_DST_REPLAY=target/reth-dst/failures/node-7-12345678.dst \
  cargo run -p reth-dst-runner --features dst --bin reth-dst-node
```

The campaign package and binary require their `dst` feature. The same feature enables Commonware
scheduling, fixed cache hashing, simulated protocol clocks and entropy, and inline parallel-work
routing in the ordinary crates that expose those boundaries. Default node builds compile native
task, cache, transport, entropy, and Rayon implementations and have no normal dependency on
Commonware, txgen, abi-fuzz, or `reth-dst`.

This slice controls workload actions, transaction count, virtual-time advances, link state, wire
corruption, node crashes, Commonware ready-batch order, and modeled MDBX results. The scheduler,
workload, and database adapter share the semantic controller, so replay checks the runnable set and
the exact storage operation at which an error occurred. Sender, envelope, recipient, and calldata
choices are still derived deterministically from the parent and branch; moving those fields into
typed controller decisions will make workload traces independent of generator implementation
changes.

Payload construction and state reads are now in-flight operations. `BeginPayloadBuild` returns the
real Engine API payload ID without waiting for a worker; `PollPayloadBuild`, virtual-time advances,
and head changes are later independent actions. `BeginStateRead` retains a provider for the current
canonical block, and `CompleteStateRead` checks its account nonces and state root after other work
and persistence have progressed. These are instances of one general rule: an accepted operation
must either complete against the state captured at its start or return an explicitly modeled
terminal result. The same begin/poll/complete shape should be added for proof requests, persistence
commands, RPC reads, and downloads instead of adding actions named for known races.

## Architecture

```text
campaign loop
  corpus and budgets
  Explore or Replay decision source
  failure artifact writer
                 |
                 v
Reth simulator
  semantic decision controller
  workload/reference model
  virtual clock
  Engine and peer message queues
  node lifecycle and fault state
  invariant checker
                 |
                 v
Commonware deterministic executor
  ready cooperative tasks --------> scheduler decisions
  virtual timers ------------------> time decisions
                 |
                 v
simulated Reth nodes
  engine + payload builder + txpool
  EVM + sparse trie + proof workers
  persistence + provider
  downloader + progressively more P2P
```

Every meaningful nondeterministic choice passes through one controller. Production constructors
keep their native implementations; simulation constructors receive controlled time, scheduling,
network, storage, and randomness capabilities. Deterministic hash seeds used only for cache layout
may remain fixed if the layout is not a tested behavior. Randomness that affects protocol or
observable results is a decision.

### Decision API

The trace schema needs stable semantic identities, not source locations, memory addresses, or
debug strings. A workable core is:

```rust
struct DecisionPoint {
    id: DecisionPointId,
    virtual_time: SimTime,
    state_digest: B256,
    options: Vec<OptionDescriptor>,
}

struct DecisionPointId {
    domain: DecisionDomain,
    actor: ActorId,
    generation: u32,
    occurrence: u64,
}

enum DecisionDomain {
    Workload,
    Schedule,
    Timer,
    EngineDelivery,
    PeerDelivery,
    NodeLifecycle,
    Storage,
}

trait DecisionSource {
    fn choose(&mut self, point: &DecisionPoint) -> Result<OptionId, ReplayError>;
}
```

`ActorId` identifies roles such as `node-1/engine`, `node-1/account-proof-0`, or
`link/node-0/node-1`. `generation` changes when a node or actor restarts. `occurrence` is local
to that stable actor and decision domain. An option has a stable ID and a typed payload. The point's
state digest covers the sorted option IDs and the small piece of model state that makes them legal.

The controller has two initial implementations:

- `Explore` selects legal choices from deterministic input bytes and records every point, option,
  and concrete generated payload.
- `Replay` consumes a recorded trace, verifies the point identity and state digest, verifies that
  the option remains legal, and applies the recorded payload.

The artifact contains the schema version, Reth and dependency revisions, chain configuration,
genesis digest, feature configuration, decision trace, terminal observations, property failure,
and logs needed to diagnose the first divergence. Store it in a compact binary encoding and supply
an inspector that prints a readable timeline. Do not use NDJSON in the execution loop.

### Commonware scheduler change

Commonware currently drains all ready tasks, shuffles them with its RNG, and polls each once. A
custom byte-stream RNG can reproduce that shuffle, but it cannot say which tasks were legal or why
a replay diverged. Extend the deterministic runtime with a scheduling policy invoked with the
stable labels and IDs of the ready tasks. The default policy retains the existing seeded shuffle;
Reth supplies a policy backed by `DecisionSource`.

The existing Commonware task ID is a global spawn counter, and its metric label is only the context
name and execution kind. Neither is a stable semantic identity when an alternate schedule changes
spawn order or several workers share a name. The hook must therefore let the caller attach a
schedule key when spawning a task, or derive one from a unique context path plus a role-local
generation. The numeric executor ID remains an internal lookup key and must not appear in a replay
artifact as the task identity.

The implemented hook is published at
`0xalpharush/monorepo@9efcb23c8f3ab91164c755ba3c87aa195717d021` while it is prepared for
upstreaming. `SchedulingPolicy` receives virtual time and the ready batch. Each runnable item has
its supervisor name and a generation local to that name; the internal executor token remains
private. The Reth policy repeatedly selects one remaining item to form a permutation, recording
each selection through the same strict semantic trace used by workload decisions.

The deterministic executor coalesces repeated wakes while a task is already queued, then makes it
eligible for one new wake immediately before polling. This matches the scheduling contract used by
Tokio and prevents a self-waking future from manufacturing thousands of duplicate scheduling
choices. A campaign timeout caused by duplicate ready entries is a simulator defect, not evidence
of a product stall. Focused tests cover both semantic ordering and duplicate-wake coalescing; the
Commonware runtime suite passes with 839 tests.

The first hook only needs to choose an ordering of the ready batch. It need not implement partial
order reduction or preempt synchronous Rust code. Existing yield points around message receipt,
timer expiry, proof jobs, persistence actions, and engine steps supply useful interleavings. Add a
new yield point only when a measured blind spot or a historical mutation requires one.

The scheduler, workload, network, and fault logic must not consume one shared opaque RNG. They ask
the controller for independently named semantic decisions. This prevents one additional workload
draw from changing the entire later task schedule.

## Stateful workload

The workload is an online state machine, not a cyclic trie round and not a serialized initial trie
profile. It owns a compact reference view of funded accounts, nonces, deployed contracts, known
blocks, canonical/safe/finalized heads supplied to Reth, node lifecycle, and submitted transaction
status. From that state it offers legal actions such as:

```rust
enum WorkloadAction {
    SubmitTx(SignedTx),
    RequestPayload { node: NodeId, parent: BlockId },
    DeliverPayload { node: NodeId, payload: PayloadId },
    UpdateForkchoice { node: NodeId, head: BlockId, safe: BlockId, finalized: BlockId },
    Connect { a: NodeId, b: NodeId },
    Disconnect { a: NodeId, b: NodeId },
    PauseNode(NodeId),
    CrashNode(NodeId),
    RestartNode(NodeId),
    AdvanceTime(Duration),
    Observe(Observation),
}
```

The valid path dominates: funded senders, correct nonces and signatures, valid parents, and legal
fork-choice ancestry build histories thousands of actions long. A separate bounded adversarial
choice can duplicate or stale a transaction, mutate an Engine or ETH message, present an old fork,
or use boundary-sized values. Invalid choices remain intentional and typed.

Use `txgen` and `abi-fuzz` as libraries to construct a transaction when the controller chooses a
transaction action. Streaming is not required. The model can request one transaction or a small
batch, update its nonce and account state after the observed result, and record the complete signed
transaction in the trace. Existing txgen binaries and output formats remain unchanged.

The local txgen commits `b26765a` and `bb80512` add whole-run `Vec` and typed-sink generation APIs.
Neither is required for this simulator. It uses txgen-core's existing `AccountManager`,
`NonceTracker`, and `BuildContext` APIs and signs Reth's consensus transaction types directly. Use a
path dependency while proving this integration, then publish the txgen-core revision before
replacing the path. Existing txgen binaries and output formats do not change.

The workload library must not create its own long-lived `StdRng`. When it needs a sender, amount,
transaction family, calldata shape, or boundary value, it obtains a labeled decision or entropy
field from the controller. Replay uses the concrete transaction in the trace, so generator changes
cannot silently reinterpret an old failure.

Run an epoch through four controller states:

1. **Build:** valid transactions and blocks establish account, contract, storage, trie, and
   persistence history.
2. **Perturb:** traffic continues while scheduling, delivery, lifecycle, and storage faults are
   enabled within the configured fault assumptions.
3. **Stabilize:** choose the nodes whose progress will be checked, restore only the dependencies
   promised by that liveness property, and freeze the remaining fault state. For a single Reth
   node, this means the node and storage remain available and the simulated CL supplies valid
   input; unrelated peers may remain down or partitioned.
4. **Check:** continue valid work or quiesce, as the property requires, within virtual-time and
   decision budgets. Run expensive recovery and convergence properties.

The outer process is a continuous loop like TigerBeetle's `cfo`: run cases until a host deadline,
print one line per property failure, retain the artifact, and continue. Each epoch is bounded by
decision count, virtual time, queued messages, blocks, memory, and output size. No txgen duration
setting determines campaign duration.

## Network, storage, and lifecycle boundaries

Start at boundaries already present in #27017 rather than at JSON-RPC sockets:

- the simulated consensus layer sends `newPayload` and `forkchoiceUpdated` messages directly to
  the production engine handler;
- the payload service and transaction pool receive typed requests;
- peer adapters exchange typed ETH requests and responses through controlled mailboxes; and
- the persistence service receives production `PersistenceAction` values.

The network controller owns pending messages. A delivery point lists their stable message IDs and
legal outcomes: deliver now, delay to a virtual deadline, duplicate, drop, corrupt through a
protocol-aware mutator, or change link state. The trace records the chosen message and outcome.
This first tests engine, download, sync, and gossip logic without depending on Tokio sockets.
Progressively move the boundary downward through ETH framing, sessions, and discovery when those
components accept simulated transport and clock capabilities.

Storage has two levels:

1. At the persistence-action boundary, choose when an operation completes, whether it returns a
   typed error, and whether the node crashes before or after its acknowledgement. This exercises
   overlay/persisted frontiers, anchors, pruning, retry, shutdown, and restart.
2. Wrap Reth's database/provider traits so each transaction, cursor, read, write, commit, and sync
   receives a stable operation ID and a controlled result. Synchronous MDBX work remains atomic to
   the cooperative executor, but errors can be injected blindly at arbitrary database operations.

The current MDBX adapter covers read and write transaction creation, direct gets, puts, appends,
deletes, clears, read and write cursor creation, table entry counts, and transaction commit. It
does not yet intercept each movement of an already-created cursor, static-file operations,
RocksDB operations, or kernel durability. One quarter of cases select a recorded operation family
(any, transaction, direct read, cursor, write, or commit) and an ordinal without naming a table.
Every observed operation is recorded; only matching operations consume the ordinal. Up to three
operations fail at separately chosen points. Expected engine termination is modeled as a terminal
result, and the next action cold-restarts the follower before another fault can be armed. The
campaign then checks convergence, storage consistency, account state, canonical hashes, and the
persisted state root. Post-rebase qualification covered direct reads, read and write transactions,
cursor creation, writes, commits, and proof-worker failures in both cooperative and native-worker
lanes.

Application simulation cannot faithfully model torn sectors, kernel writeback, mmap behavior, or
MDBX's response to a power cut by returning an error from a Rust trait. Those belong in native
crash tests and the later Bedrock block-device lane. Keep the distinction explicit in coverage
reports.

A simulated crash drops a node's tasks and volatile state without running graceful shutdown, keeps
only state designated durable by the storage model, increments the node generation, and rebuilds
the production components. Pause and resume affect which actor tasks are runnable. Restart is a
logical action with a deterministic completion event.

## Snapshots and branching

Do not require arbitrary in-memory snapshots for the first campaign. Rust future stacks, pending
Tokio/Commonware tasks, Reth engine state, open MDBX transactions, and network connections are not
serializable. Commonware's existing restart checkpoint deliberately excludes most of those values.

The correct first branch representation is:

```text
genesis + decision prefix [0..k] + alternate legal decision k+1 + continuation
```

Rebuild a branch by replaying the prefix. This is slower than copy-on-write memory but simple and
correct. Retain prefix digests and compare the reconstructed canonical simulator hash so replay
cannot drift silently.

Add cached checkpoints only at quiescent reconstruction boundaries where all node tasks have
stopped and state has an explicit representation: closed durable database state, workload model,
virtual clock, pending external messages, topology/fault state, and trace prefix. Resume by
reconstructing actors. Arbitrary mid-task branching requires either refactoring those actors into
serializable explicit state machines or snapshotting the whole VM. Bedrock supplies the latter;
it should not block the earlier campaign.

Initially explore independent runs and mutated trace prefixes rather than branching every event.
When replay cost becomes measured campaign bottleneck, add quiescent checkpoints. Copy-on-write is
an optimization after the state boundary is proved correct.

## Properties and oracles

No one reference is independent enough. Retain a failure when any applicable property fails.

### Execution and trie differential

Feed each accepted logical block into a simple reference profile that disables the optimized
state-root task. Compare payload status, block hash, state root, receipts root, logs bloom, gas used,
recovered transactions, balances, nonces, code, and sampled storage. Periodically rebuild the
reference from genesis. Compare with another execution client or EVM implementation where the
interface is available; serial Reth alone shares too much code to catch every error.

### Durable acknowledgement

Assign a logical index to every externally acknowledged canonical transition. Track the durable
frontier separately from the overlay head. After any crash or storage error, recovered database,
static files, checkpoints, overlay anchor, and reconstructed root must describe one legal prefix.
A mixed generation, lost acknowledged transition, or rewind through a preserved anchor is a
failure.

### Engine and cross-node safety

A valid payload derived from the reference state must not be rejected, and an invalid state or
receipts root must not be accepted. For the same accepted block, independent nodes must agree on
execution results. After faults stop and nodes receive the same valid block graph and fork-choice
sequence, they must converge within the budget. Finalized ancestry supplied by a valid simulated CL
must remain monotonic.

### Metamorphic and progress checks

Duplicate delivery, allowed message reorder, a clean restart, replay of an accepted payload, and
rebuilding from the accepted block log must preserve the relevant result. Once faults stop, payload
processing, proof workers, persistence, download, sync, cancellation, and shutdown must either
complete or return a terminal error within virtual budgets. A stall is a property failure with the
last runnable tasks, messages, storage operations, and external responses recorded.

## Exploration and historical qualification

Start with random legal decisions, broad semantic state feedback, and full trace replay. Useful
feedback includes new Engine status transitions, DAG shapes and reorg depths, persisted/overlay
distances, proof worker completion orders, lifecycle/recovery states, peer/sync states, property
sites, and Reth-scoped code coverage when it is cheap to collect. Do not begin with MCTS, exhaustive
partial-order reduction, compiler memory instrumentation, or trace shrinking.

Historical bugs are a mutation gate, not workload templates. For each buildable pre-fix revision,
run the same generic campaign and ask whether its actions, schedule, fault boundaries, and oracles
rediscover the failure. The required general capabilities are:

| Bug family | Generic capability |
| --- | --- |
| Preserved anchors and persistence lag | Generated fork DAG, independent durable/overlay progress, crash/restart, prefix and root oracle. |
| Overlapping proof targets and stale branch masks | General account/storage changes across partially persisted blocks, cooperative proof scheduling, serial root differential. |
| Retention, pruning, and payload/persistence races | Long valid histories, reorg/prune pressure, persistence actions concurrent with engine work, rebuild oracle. |
| Empty accounts and hashed-state conversion | Transfers, account lifecycle, contracts, storage, and protocol-valid deletion semantics. |
| Late hints, stalled proofs, low-core deadlocks | Multiple cooperative proof workers, controlled runnable-task order, cancellation and progress oracle. |
| Update/removal cross-cancellation | Repeated generated state transitions and observations before update drain, external root and persisted-state differential. |
| RPC proof conventions | A later RPC action boundary plus cross-client and metamorphic response checks. |
| Provider reuse after errors | Blind typed failures at arbitrary provider operations, retry/restart, terminal-error and recovery properties. |
| Weak-memory or true simultaneous races | Native multicore lane; these are outside the cooperative simulator's proof. |

Do not add an action named for a historical issue, a trie threshold profile, a database table, or a
known corrupt offset. A deliberately seeded mutant is useful only to prove that a generic property
and exploration mechanism can detect its class.

If semantic exploration later plateaus on concurrency, use the approach demonstrated by
[SECT](https://www.usenix.org/conference/usenixsecurity26/presentation/xu-jiacheng) and
[MAccConc](https://projectzero.google/2026/09/maccconc-race-condition.html): instrument
synchronization or selected shared-memory accesses, identify events with stable count-augmented
stacks, and offer bounded preemption decisions at discovered communication points. Measure the
need before adding this machinery.

## Why this direction matches prior systems

[FoundationDB simulation](https://apple.github.io/foundationdb/testing.html) runs a complete
cluster in one deterministic, single-threaded actor runtime and models networks, disks, machines,
datacenters, time, and faults. Its separate multithreaded API tests cover race classes that the
simulator intentionally cannot. This is the same application/native split proposed here.

[TigerBeetle's simulator](https://github.com/tigerbeetle/tigerbeetle/tree/main/src/testing) models
cluster, packet, storage, time, and state-machine behavior as application components. Its continuous
campaign tooling is the operational model for the Reth loop. TigerBeetle's
[protocol-aware DST](https://tigerbeetle.com/blog/2026-08-20-protocol-aware-dst/) explains the
additional advantage of this layer: the simulator can inspect and compare internal consensus and
storage invariants at every transition, rather than waiting until a violation reaches a public API.
Its [liveness mode](https://tigerbeetle.com/blog/2023-07-06-simulation-testing-for-liveness/)
stabilizes only a selected healthy core and makes other faults permanent, which avoids accidentally
hiding stalls by eventually healing or restarting everything. Its separate Vortex harness provides
nondeterministic full-system coverage.

[Dropbox Nucleus](https://dropbox.tech/infrastructure/-testing-our-new-sync-engine) is the closest
Rust execution precedent. Nucleus's control plane is a top-level Rust `Future`; its Trinity harness
alternates between polling that future and completing intercepted filesystem, network, and timer
requests. It injects failures, reorders completions, simulates crashes, checks convergence, and
reruns the final state for determinism. Dropbox separately uses native-filesystem and real-protocol
tests because the fast simulator replaces those implementations. #27017's `TaskRuntime` and
cooperative Reth components provide the corresponding control-plane seam.

[Turso's simulator](https://github.com/tursodatabase/turso/blob/main/testing/simulator/README.md)
uses a stateful model to generate long valid interaction plans and can compare the same plan with
SQLite. Turso also documents the limit clearly: its application simulator replaced the actual I/O
loop, while Antithesis found an `io_uring` partial-write bug in that omitted layer. This supports
using an application simulator first and a whole-system executor for native I/O, rather than
claiming either layer is sufficient alone. See
[Turso's DST discussion](https://turso.tech/blog/introducing-limbo-a-complete-rewrite-of-sqlite-in-rust).

[Antithesis's Chain of Blocks](https://antithesis.com/docs/resources/chain-of-blocks/) separates a
continuous state-building workload from safety and eventual-progress properties. Antithesis's
[deterministic hypervisor](https://antithesis.com/blog/deterministic_hypervisor/) then supplies
whole-environment determinism for unmodified native systems. Reth should use the cheaper
application control already enabled by #27017 before requiring that environment.

[CHESS](https://www.usenix.org/event/osdi08/tech/full_papers/musuvathi/musuvathi_html/index.html)
shows that useful concurrency search can be bounded at synchronization and asynchronous events;
many bugs require few preemptions. [SKI](https://www.usenix.org/conference/osdi14/technical-sessions/presentation/fonseca)
and [SMP-ReVirt](https://pdos.csail.mit.edu/6.828/2010/readings/smp-revirt.pdf) show why true
deterministic SMP is a separate shared-memory ordering problem rather than a prerequisite for an
application simulator.

[Fuzzamoto](https://github.com/oss-garage/fuzzamoto) demonstrates a full-node campaign that keeps
the workload, feedback, artifact, and executor layers separate. Its
[Bedrock backend issue](https://github.com/oss-garage/fuzzamoto/issues/142) reaches the same
practical conclusion: keep the campaign portable and treat Bedrock as one backend. Ethereum
[Attacknet](https://ethpandaops.io/posts/attacknet-introduction/) supplies realistic later network
topologies and fault scenarios, but not deterministic replay.

[Tuner](https://github.com/tempoxyz/tuner/) and the local txgen property-runner branch provide useful
patterns for LibAFL corpus scheduling, model-free continuous cases, ABI-aware transaction
generation, differential execution, and compact failure artifacts. Adopt those pieces only after
the semantic controller works; none is the deterministic executor.

## Implementation milestones

### Milestone 1: semantic record/replay

Items 1 through 4 now have an executable first slice. Schedule identity remains valid only for
cooperative Commonware tasks; native Tokio, Rayon, MDBX, and EVM work is still atomic or outside
the recorded scheduler. Item 5 remains campaign qualification work.

1. Define stable actor, message, storage-operation, decision-point, option, and trace types in a
   small `reth-dst` crate.
2. Add `Explore` and strict `Replay` decision sources plus a binary artifact inspector.
3. Add the Commonware ready-task scheduling-policy hook. Prove that default Commonware behavior is
   unchanged and that a recorded schedule replays with legality checks.
4. Convert #27017's fixed node scenario into generated Engine, transaction, fork-DAG, delivery,
   observation, and lifecycle actions. Generate transactions on demand through txgen/abi-fuzz.
5. Run at least 1,000 decisions per epoch and 100 identical replays of selected nontrivial traces.
   Require identical semantic observations and a useful error at the first deliberately altered
   decision point.

The milestone fails if stable actor/message identities require process addresses, if replay cannot
identify its first divergent decision, or if production component code must be duplicated in the
simulator.

### Milestone 2: generic trie and persistence campaign

Items 1, 2, the blind MDBX portion of item 3, and the continuous runner in item 5 have executable
first slices. Persistence completion control, cursor-movement faults, static-file/RocksDB faults,
historical rediscovery measurements, and longer qualification campaigns remain.

1. Add serial execution/trie differential and durable-prefix oracles.
2. Generate long account, contract, storage, fork, reorg, partial-persistence, prune, cancel, and
   restart histories without issue-specific profiles.
3. Add persistence completion/error decisions, then blind provider fault wrapping.
4. Run historical pre-fix revisions and publish rediscovery rate and decisions-to-failure for every
   deterministically reproducible catalogue entry.
5. Run continuously in a `cfo`-style process that prints failures and continues.

### Milestone 3: network and native fidelity

1. Expand from the current downloader peer adapter through ETH framing, sessions, tx/block gossip,
   and multi-node sync using controlled in-memory transports.
2. Run the same workload traces and oracles against native multicore nodes. Preserve failures even
   when only the external trace, not the native thread schedule, is replayable.
3. Qualify Bedrock for whole-node process, kernel, mmap, filesystem, and native-thread tests. Add a
   block model only after Reth boots and same-input VM replay is demonstrated on supported hardware.
4. Use Hermit for native replay where it is faster and compatible. Use Attacknet for larger
   nondeterministic multi-client validation.

Only after these campaigns expose a measured replay or exploration bottleneck should the project
add quiescent snapshots, copy-on-write branching, LibAFL coverage scheduling, shrinking, or
compiler-discovered scheduling points.

Run the native-worker differential lane with an explicitly qualified binary:

```console
cargo build --release -p reth-dst-runner \
  --features dst,native-differential --bin reth-dst-node
RETH_DST_NATIVE_WORKERS=1 RETH_DST_SECONDS=3600 RETH_DST_STEPS=1000 \
  target/release/reth-dst-node
```

The semantic workload, network, storage-fault, and lifecycle decisions remain recorded. Native
Rayon worker ordering is intentionally outside the trace, so a native-only failure must reproduce
statistically and receive an independent component-level false-positive check before it is
reported as deterministic.
