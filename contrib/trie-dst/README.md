# Trie pipeline DST

This workload drives reth's sparse-trie task and proof workers against a real
empty-state database and `OverlayManager`, with concurrent persistence through
`DatabaseProvider::save_blocks`. It does not start an execution client or execute
transactions.

All harness files live here. The only production changes expose the existing
state-root task entrypoint, its options and pending-trie publication function,
the engine's changeset-cache retention constant, and the provider factory's
shared overlay manager.

## Build and run locally

From the reth repository root:

```sh
cargo build --locked -p reth-trie-dst --profile dst
target/dst/reth-trie-dst '{"seed":42,"blocks":64,"datadir":"/tmp/trie-42"}' > /tmp/reth-trie-dst-42.log
```

Prerequisites for native builds are reth's normal Rust/native build dependencies,
including Clang for RocksDB's bindings. The crate belongs to the root workspace and uses its
shared dependencies, `Cargo.lock`, and the `dst` profile. Keep `dst` for this harness:
it inherits `profiling` while enabling debug assertions and overflow checks.

The executable accepts one optional JSON object:

| Field | Default | Range |
| --- | --- | --- |
| `seed` | 0 | unsigned 64-bit integer |
| `input` | generated from seed | explicit operation stream ending each block with `Commit` |
| `datadir` | temporary directory | persistent database and journal directory; reuse to restart |
| `blocks` | 64 | 1–1024 |
| `updates` | 64 | 1–4096 per block |
| `proof_workers` | 2 | 1–16 per account/storage pool |
| `timeout_secs` | 60 | 1–3600 per task result or persistence acknowledgement |
| `persistence_threshold` | 50 | retained canonical block count must exceed this to start persistence |
| `num_state_masking_blocks` | 30 | persisted suffix whose state/trie writes remain masked |
| `memory_block_buffer_target` | min(5, threshold) | blocks to retain above the database tip; optional/null uses Reth's default |

These are the defaults in this Reth revision. As in Reth, the buffer must not
exceed the persistence threshold. With masking enabled, masking blocks plus the
buffer must be strictly less than the threshold. A zero threshold disables
masking. The backpressure threshold follows Reth's default, max(16, 2 × threshold).

Each block's input operations and persistence frontiers are printed before its
result is awaited. The final JSON line reports completion, commits, partial
commits, and counts from production metrics. A mismatch or timeout exits
unsuccessfully. Keep stdout and stderr.

## What runs

The generator draws new addresses and storage slots uniformly from the full
32-bit range (0 through `0xFFFFFFFF`). It uses up to 16 generated accounts,
reusing existing accounts and slots for storage-only updates. It creates
nonempty accounts, changes nonce/balance, overwrites slots, and deletes
individual slots. Each block receives the configured number of
updates. It independently tracks the complete state and builds a per-block
`BundleState` with original and final account/storage values and reverts.

The input is an ordered stream of `UpdateBatch`, `PrewarmBatch`, and `Commit`
operations. Each update batch contains account and storage updates in write
order; each prewarm batch contains account addresses and storage keys as
access hints. `Commit` finishes the sparse-trie task, checks the reference
root, and inserts the resulting block. The input records the batch order
explicitly, so replay needs no stream seed. Prewarm batches produce
`LeafUpdate::Touched` entries without changing state.
Hints include a random nonempty subset of distinct written keys plus read-only
accesses to unwritten accounts/storage. They overlap actual updates without
always covering them, and may arrive before or after an overlapping write.
Batch composition and hint targets are logged; the completion summary counts
update and hint batches. The generator uses the seed to construct the initial
stream. At each `Commit`, the root must equal a root rebuilt from the reference
maps with `HashBuilder`:
storage leaves first, followed by RLP account leaves. Its actual hashed state
and trie updates are attached to the synthetic executed block alongside the
bundle, then inserted into the real overlay manager.

The task's pending trie is published only after the block root is accepted and
reused by subsequent tasks. Persistence acknowledgements request pruning by
the next task using the retained parent chain, newest first.

Persistence follows the engine tree's linear-chain happy path. After a block
or persistence completion, an idle writer starts a save when the retained
canonical block count exceeds `persistence_threshold`. It saves block data
through `head - memory_block_buffer_target` and state/trie data through
`new_db_tip - num_state_masking_blocks`, never moving either frontier backwards.
The retained count includes the masked suffix, matching Reth's canonical
in-memory chain. Exactly one save is in flight; subsequent blocks continue
while it runs, with Reth's normal backpressure limit. Completion is polled at
block boundaries. Finish advances through the production write path. Only after
acknowledgement are persisted overlays removed through the state/trie frontier,
the execution overlay precomputed at its new anchor, and trie pruning requested.
At shutdown, the worker finishes any active save and fully persists the head,
matching Reth's shutdown flush.
It then drains trie, proof-worker, and deferred-drop queues before exiting, so
all database handles close while RocksDB's global scheduler is still alive.

After every persistence commit, a fresh overlay-backed provider must expose the
reference accounts and every touched slot, including deleted slots, plus both
range endpoints at the database tip. Fully flushed commits additionally compare
the complete raw hashed tables with the reference and calculate a fresh
database-backed root. Metrics report proof revelation, trie reuse, pruning,
overlay computation, and partial commits. Their counts depend on the selected
persistence settings and run length.

The chain is linear. Reorgs, EVM execution, arbitrary account
destruction and storage wipes are excluded. In particular, the generator does
not invent storage wipes outside valid CREATE2/SELFDESTRUCT semantics.

## Run under Bedrock

Use a Bedrock checkout with the generic workload runner and its documented host
requirements (Docker, Nix, and the Bedrock kernel/device setup). From that checkout:

```sh
RUN_DIR=/tmp/reth-trie-inputs ./scripts/dst/run.sh \
  --workload /absolute/path/to/reth/contrib/trie-dst/workload \
  --workload-arg blocks=64 --workload-arg proof_workers=2 \
  --duration 10m --workers 16 --out /tmp/reth-trie-runs
```

The preparation hook builds the DST executable and controller image,
compiles the readiness helper against Bedrock's `guest/libvmcall.h`, and
normalizes the image archive with Bedrock's archive tool. It keeps symbols in
the original executable and strips debug sections only from the image copy.
The inline builder supplies Rust, Clang, sccache, and mold; the host needs Docker,
Python 3, and `flock` on x86-64 Linux. `DOCKER` and `CARGO_BUILD_JOBS` customize
the build tools.

Cargo downloads, compiled targets, and the unstripped executable persist under
`BEDROCK_DST_BUILD_CACHE` (Bedrock CI's workload cache), defaulting to
`${XDG_CACHE_HOME:-$HOME/.cache}/reth-trie-dst`. Compilation uses sccache and mold
with the `dst` profile. `BEDROCK_DST_SCCACHE_DIR` selects the compiler cache, defaulting
to the workload cache's `sccache` directory; `SCCACHE_CACHE_SIZE` defaults to `10G`.
Preparation writes timings and cache paths to `RUN_DIR/cache-info.env` and
compiler statistics to `RUN_DIR/sccache.txt`, which Bedrock CI displays and
uploads with its artifacts.

The controller implements `parse-args`, `config`, `warmup`, `start`, `progress`,
`finalize` and `observe`. It starts the executable under Bedrock's `thread-fuzz`,
so the scheduler varies the real task/proof/persistence interleavings. By default,
each seed resumes the same live initialized worker and empty database from the
shared warm checkpoint; `cold_start: true` instead creates them after branching. The seed
controls generated operations and the recorded crash plan; Bedrock's recorded
random/preemption streams control scheduling. Mid-run semantic decision editing
is unsupported.

The Bedrock-only `crashes` argument defaults to 1 (range 0–16, at most
`blocks`). Set it to 0 for the happy path. The controller draws distinct block
numbers uniformly across the entire run, from block 1 through the final block.
When it observes the corresponding block-input event, it sends SIGKILL to the Rust
process and immediately restarts it against the same directory. The worker
continues running after emitting that event, so the kill can interrupt trie
computation or concurrent persistence. These are process crashes: the kernel
and its page cache survive. Container restarts and power-loss faults are outside
this workload.

Before processing each new block, Rust appends its operations to `blocks.jsonl`.
Startup reads the journal into block-number order, deduplicates identical records,
rejects conflicting records, and ignores incomplete JSON left by an interrupted
append. A new append starts on a fresh line. Operations are read back rather than
regenerated, including blocks that must be replayed after an unwind.

Startup uses `LaunchContext::create_provider_factory_with_recovery`, the real node
startup path. It checks storage consistency and unwinds the production pipeline
when `partial_state_trie < Finish`. Reth's durable unwind marker also handles a
second crash during recovery. The actual committed checkpoint determines the
recovered head, including commits whose acknowledgement was lost. The reference
state is reconstructed from the journal through that head and compared with the
recovered header root, hashed tables, and storage. All
volatile overlays and the sparse trie start cold; generation then resumes at the
next block. Keep the same seed and configuration when reopening a directory.

The run log records pre-recovery Finish/state-trie frontiers, the recovered head
and root, each kill, and the existing block/persistence events. `done.json` reports
armed and fired kill counts and completed recoveries. A missed kill, unexpected
process exit, failed recovery check, or failure to finish within the seed horizon
fails the run.

Finalization reports unfinished work as a failure and collects `reth-trie-dst-run.log`,
`reth-trie-dst-controller.log`, `reth-trie-dst-done.json`, and the append-only
`reth-trie-dst-blocks.jsonl` under `/bedrock/out` alongside
the merged assertion stream. A worker timeout also fails the run. Increase the
driver's per-seed virtual horizon (`--run-secs`) if a larger configuration cannot
finish before finalization.

Bedrock's shared runner owns the host wall-clock budget: `--duration 10m`
includes guest boot and warmup, and optional `--num-seeds 100` caps the total
seeds across all workers. Omit the cap to run until the deadline. Each seed still
executes its configured number of blocks. The deadline stops in-flight seeds and
reports them as interrupted; they do not count as passing.

For CI, use Bedrock's DST workflow with this external workload source, replacing
`RETH_REF` with a branch, tag, or commit:

```sh
gh workflow run tempo-e2e.yml --repo tempoxyz/bedrock \
  -f workload='git::https://github.com/paradigmxyz/reth.git//contrib/trie-dst/workload?ref=RETH_REF' \
  -f workload_config='{}' -f duration=10m -f parallel_seeds=16
```

CI's `duration` is the same host budget; `num_seeds` is a global cap, unlimited
when blank. The workflow fetches the selected Reth revision. Preserve `RUN_DIR`
and use the generic runner's `--prepared` option to reuse the exact inputs. For an actual recorded schedule, use Bedrock's replay command on the
recorded run; rebuilding an image or changing the binary changes the World.

## Fuzz explicit trie inputs

With the accompanying Bedrock workload-input hooks, set `DST_CMD=fuzz` during
preparation and execution. For example, dispatch the same CI workflow with
`mode=fuzz`, `parallel_seeds=1`, and this workload URL. The workflow's
`num_seeds` then caps fuzz executions. Each corpus entry is an `Input`: an
ordered operation stream. Reth generates eight initial entries and exposes
typed AST sites for LibAFL to choose. Its operators edit account/storage
fields, prewarm keys, nested updates, batches, and commit boundaries; crossover
draws a second corpus entry. Invalid edits leave the input unchanged. The same input can
also be supplied as `base_input` to the controller for replay.

At each completed block, the controller emits `trie/input/checked`, which gives
Bedrock a pull point when mid-run snapshots are enabled. A suffix input is
injected at the next new block boundary; already journaled blocks are never
replaced. The worker waits after its original input until Bedrock either injects
a suffix or finalizes the run. This workload sets `--max-snaps=0` in
`bedrock.yaml`, so fuzzing currently uses only whole-input mutations from the
warm checkpoint; the suffix path remains available for experiments. Fuzz
preparation builds a coverage-instrumented guest executable and keeps an
uninstrumented executable beside the host controller for input hooks.
The first scope is trie operation streams on a linear
chain; it does not mutate Reth code or the VM's recorded scheduling decisions.

### Warm initialization

The default controller warmup starts a live worker from fixed `workload_args`,
initializes its runtime, empty database, storage and genesis, and checks a
Unix-datagram readiness handshake before returning `ready: true`. Repeated hooks
poll the same process; dead workers and initialization exceeding 120 guest seconds
are errors. The worker blocks in bounded socket I/O (one-hour guest deadline), not
an input-dependent polling loop. No corpus input, generation or expected roots are
loaded in this warm prefix.

After branching, `start` sends the execution config path over that socket. The
worker validates that proof-worker counts, storage directory and persistence/tree
settings match the initialized configuration, then parses and runs the input and
its normal initial assertions. Guest-local `stdout.log` retains output across hook
processes; `init.log` and `warm-ready.json` capture the idle worker PID and output
frontier. Adoption seeks past that frontier and emits a `warm: reused` PID event,
so pre-checkpoint recovery output is not counted as per-execution recovery; `run.log` and the controller's SIGKILL/recovery monitor remain active.
Subsequent crash restarts launch normally and reopen the real persisted database,
without an initialization barrier. Continuation input and synchronous
`run_secs: 1` execution use the same monitor as longer runs.

The current coverage frontend retains inline counters and its crash-persistent
feedback map across the warm checkpoint; the initialization barrier alone does
not remove initialization coverage from per-input feedback.

Set `cold_start: true` in workload args to retain per-execution startup coverage.

### Workload process protocol

Bedrock preparation can export `fuzz-session.json` with `--prepare-only` and
`--executor-manifest /absolute/run/fuzz-session.json` for SDK-free planner and
executor consumers. The planner adds version discovery and bounded deterministic
structural reduction while keeping the existing one-shot input hooks and
standalone executable. See [the public protocol contract](workload/PROTOCOL.md)
for argv, JSON schemas, validation semantics, and whole-input limitations.
