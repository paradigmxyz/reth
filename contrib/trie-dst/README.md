# Trie pipeline DST

This workload drives reth's sparse-trie task and proof workers against a real
empty-state database and `OverlayManager`, with concurrent persistence through
`DatabaseProvider::save_blocks`. It does not start an execution client or execute
transactions.

All harness files live here. The only production changes expose the existing
state-root task entrypoint, its options and pending-trie publication function,
and the provider factory's shared overlay manager.

## Build and run locally

From the reth repository root:

```sh
cargo build --locked -p reth-trie-dst --profile dst
target/dst/reth-trie-dst '{"seed":42,"blocks":64}' > /tmp/reth-trie-dst-42.log
```

Prerequisites for native builds are reth's normal Rust/native build dependencies,
including Clang for RocksDB's bindings. The crate belongs to the root workspace and uses its
shared dependencies, `Cargo.lock`, and `dst` profile. That profile inherits
`profiling` and enables debug assertions and overflow checks.

The executable accepts one optional JSON object:

| Field | Default | Range |
| --- | --- | --- |
| `seed` | 0 | unsigned 64-bit integer |
| `blocks` | 64 | 16–1024 |
| `updates` | 64 | 1–4096 per nonempty block |
| `proof_workers` | 2 | 1–16 per account/storage pool |
| `timeout_secs` | 60 | 1–3600 per task result or persistence acknowledgement |

Each block's input operations and persistence frontiers are printed before its
result is awaited. The final JSON line reports completion, commits, partial
commits, cold starts, and counts from production metrics. A mismatch, timeout,
or missing component effect exits unsuccessfully. Keep stdout and stderr.

## What runs

The generator uses 16 accounts and 64 slots per account. It creates nonempty
accounts, changes nonce/balance, performs storage-only updates, overwrites slots,
and deletes individual slots. Every eleventh block is empty. It independently
tracks the complete state and builds a per-block `BundleState` with original and
final account/storage values and reverts.

The production task consumes streamed hashed updates. At each block boundary,
its root must equal a root rebuilt from the reference maps with `HashBuilder`:
storage leaves first, followed by RLP account leaves. Its actual hashed state
and trie updates are attached to the synthetic executed block alongside the
bundle, then inserted into the real overlay manager.

The task's pending trie is published only after the block root is accepted.
Subsequent tasks reuse it and prune using the retained parent chain, newest
first. Every seventh block starts cold, forcing proof loading through the
normal overlay provider factory.

Every third block submits persistence after its first update, before its stream
is finished. A background worker writes a contiguous prefix while the task and
proof workers run. Alternating saves fully flush or retain a one/two-block
masked suffix; the last save fully flushes the chain. Finish advances through
the production implementation. Overlay blocks are removed only after commit
acknowledgement, through the state/trie frontier.

After every commit, a fresh overlay-backed provider must expose the reference
accounts and all 64 slots, including deleted slots, at the database tip. Account
and storage proofs must verify against the independent root. Fully flushed
commits additionally compare the complete raw hashed tables with the reference
and calculate a fresh database-backed root. Completion requires observed proof
revelation, trie reuse, pruning, overlay computation, and partial commits.

The chain is linear. Reorgs, crash recovery, EVM execution, arbitrary account
destruction and storage wipes are excluded. In particular, the generator does
not invent storage wipes outside valid CREATE2/SELFDESTRUCT semantics.

## Run under Bedrock

Use a Bedrock checkout with the generic workload runner and its documented host
requirements (Docker, Nix, and the Bedrock kernel/device setup). From that checkout:

```sh
RUN_DIR=/tmp/reth-trie-inputs ./scripts/dst/run.sh \
  --workload /absolute/path/to/reth/contrib/trie-dst/workload \
  --workload-arg blocks=64 --workload-arg proof_workers=2 \
  --seeds 8 --out /tmp/reth-trie-runs
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
so the scheduler varies the real task/proof/persistence interleavings. Each seed
starts with a new empty database after the shared warm checkpoint. The seed
controls generated operations; Bedrock's recorded random/preemption streams
control scheduling. Mid-run semantic decision editing is unsupported.

Finalization reports unfinished work as a failure and collects `reth-trie-dst-run.log`,
`reth-trie-dst-controller.log`, and `reth-trie-dst-done.json` under `/bedrock/out` alongside
the merged assertion stream. A worker timeout also fails the run. Increase the
campaign duration if a larger configuration cannot finish before finalization.

For CI campaigns, check out both reth and Bedrock on the runner and invoke the
same generic runner with this workload directory. A Bedrock-only checkout does
not contain this workload. Preserve `RUN_DIR` and use the generic runner's
`--prepared` option to reuse the exact inputs. For an actual recorded schedule, use Bedrock's replay command on the
recorded run; rebuilding an image or changing the binary changes the World.
