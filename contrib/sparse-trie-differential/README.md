# Sparse trie differential workload

This directory is a native sparse-trie differential plugin. Rift, the external
coverage-guided fuzzer (the **host**), loads this plugin. It generates and mutates
inputs, runs them, and keeps inputs that expose new native code paths or semantic
features: completed scenario names and logarithmic operation-count ranges. This
plugin is its native adapter, loaded once into each **worker** (also called a
client): a separate process that executes inputs sequentially. Its comparison logic (the **core**) is the shared
example in `crates/trie/sparse/examples/sparse_trie_differential.rs`: it compares
`ArenaParallelSparseTrie` and `PersistentArenaParallelSparseTrie`, two sparse-trie
backends, against independent checks called **oracles**. These rebuild expected
roots from an ordered map of key/value leaves and compare retained updates (node changes
accumulated since the previous reset) with a separate trie. A detected
difference is an **objective**, returned to Rift with an `objective_signatures`
category, an array of stable failure identifiers, and diagnostic details. Replaying the complete input reproduces the operation
sequence from beginning to end; successful reports include the fixture seed, operation counts and scenarios.

Each execution combines two in-memory checks: a fixed sequence of checks of the `SparseTrie` trait
(the shared backend API) on a standalone storage trie, and sixteen rounds of
explicit account/storage actions. We call the first sequence the **storage trait
spine** and the second the **action tape**. The fixture seed chooses the spine's starting leaves;
all API checks run even if mutations remove every tape action.

Inputs are version-4 JSON text: `version`, an unsigned 64-bit `fixture_seed`,
optional `clustered` range recording which rounds came from the cluster generator,
and exactly sixteen ordered action lists in `rounds`, one list per round. The seed
generates the storage trait spine; execution never
regenerates the explicit actions. Each round accepts up to thirty-two actions:
`update_batch` (up to four ordered changes), `prewarm` (look up an account or reveal
a depth-limited storage proof), `reveal` (a proof for the separate storage trait
spine), `prune` (blind old nodes), `snapshot` and `clone` (copy and check the root),
and `tracking_reset` (reset the retained-update accumulator). Proofs may reveal an
extension before its branch child; retries hydrate missing children. An update is
`account` (nonce and balance, preserving storage), `delete_account` (remove the
account and all storage), `storage` (account key, slot and value; zero deletes the
slot), or `leaf` (an update to the separate storage trait spine). Keys, slots and
values are fixed-width hexadecimal words. Changes execute in order, so a delete
followed by recreation in the same batch starts with empty storage. Mutations may
produce storage writes to absent accounts; these are explicit no-ops, not errors.
Empty batches and action lists remain valid shrinking results. Inputs are limited
to 28672 raw bytes and 30000 JSON-escaped bytes, leaving room for a second input
used as a crossover donor in the 65536-byte native request. Older versions are
rejected; the host execution seed cannot change an already serialized tape.

Generation starts with 1–4 accounts, then uses a shuffled population of 256
prehashed account keys. New accounts enter over time; hot-key episodes persist
while a cursor rotates through cold keys. Each round visits active accounts before
repeating any and interleaves updates with targeted and unrelated prewarm hints.
One third of histories remain plain; the others splice in 6–14 clustered rounds.
The cluster generator uses 24-bit indices (six nibbles), shared prefixes of 0–3
nibbles, 6–24 unique keys, and randomly resampled suffixes of existing anchor keys.
The introduced set holds keys available so far; the active set is the current
working set. Each refreshes with 25% probability. The initialization and
at least one plain round remain, within sixteen rounds and four updates per round.
An existing account has a 1/8 deletion chance before metadata/storage weighting,
including in clustered rounds. Absent accounts are recreated with nonce one;
metadata changes preserve slots. Storage updates reuse touched slots half the
time and write zero one third of the time. Deletion forgets storage and touched
slot history. Mixed splices that would write storage to a deleted account fall
back to the plain history, rather than silently resurrecting the account.

The account trie contains canonical account RLP, including each actual storage
trie's root. Independent ordered-map models rebuild storage and account roots;
an independent `alloy_trie::HashBuilder` rebuild checks account branch updates, while the existing
storage proof/update oracle checks slot changes. Raw retained updates are also
compared across backends. Account paths stay revealed; storage prewarms exercise
partial proofs. A mandatory populated-wipe/recreation case checks that clearing
a trie really yields an empty root before reinitialization can hide stale state.
Counters and features distinguish generated deletions, generated clustered
deletions and generated populated wipes from these fixed checks. Account identity
and storage are modeled, but persistence, database commits, prefix-preimage lookup
files and background-worker timing are not.

A trie "fork" here means an independent in-memory snapshot, not a blockchain
fork. Persistent snapshots share unchanged nodes; writes must not leak between
copies. A nested snapshot is a snapshot of a snapshot. Existing fixed checks
mutate sibling copies and verify survivors after their source is cleared or
dropped. Fixed extension-only checks also snapshot or clone hash-only (opaque) children, hydrate and mutate the copy, and verify the source remains unchanged. Arbitrary snapshot ancestry and drop order are not mutable tape actions.
The mandatory check sequence runs alongside generated actions. Fourteen local mutation
operators edit those actions, batches, keys, values and prune cutoffs; they are listed
in the Native calling interface section below.

Execution is in memory: no database, output files, network or subprocesses.
Preparation and Rift's result management may use disk. Every input runs on both
backends twice: once with all four parallelism thresholds set to their integer
types' `MAX`, and once with all four set to zero. The fields are
`min_revealed_nodes`, `min_updates`, `min_dirty_leaves` and `min_leaves_for_prune`.
The zero thresholds exercise
parallel proof revelation, updates, hashing and pruning when the trie contains
suitable subtries. Roots, retained updates and observable transcripts are compared
across all four runs. Transcripts include proof requests, pending changes, pruning
counts and root-cache epochs. Each execution creates a local two-thread Rayon pool after
Rift has forked its worker process, so no live thread pool is inherited across
`fork()`. All worker threads are scoped and joined before reading coverage.
It does not initialize Rayon's process-global pool. The successful report records
`execution_modes: 2` (serial and parallel) and `worker_threads: 2`; operation
counts sum all four runs.

## Prepare on the execution machine

Requirements: Linux, a C linker, Rust 1.96 or newer with LLVM SanCov support,
Cargo, Git, Python 3, `ldd`, GNU `nm`, and access to public Cargo and Git dependencies. The independent Cargo workspace
uses path dependencies into the Reth checkout containing this directory, with a checked-in lockfile and
public dependency revisions. No private SDK, container upload or registry-hosted
workload library is required.

```sh
printf '{}\n' > /absolute/workload-config.json
RUN_DIR=/absolute/new-inputs ./contrib/sparse-trie-differential/prepare.sh \
  --workload-config /absolute/workload-config.json
```

The preparation configuration accepts only `{}`. The script generates a
**session**: JSON settings telling Rift which library to load and how to search. `RUN_DIR` must be outside the checkout. Preparation refuses to replace an existing
session. Optional `CARGO_BUILD_JOBS` (default 2) and `CARGO_TARGET_DIR` control
build resources, not execution concurrency. The script compiles the exact supplied
tree **on the execution machine**, using `--locked --profile fuzz`. The independent
`fuzz` profile inherits release optimizations, enables debug assertions and overflow
checks, retains full debug symbols, and uses unwinding panics. Preparation copies
the library from the target directory's `fuzz/` subdirectory and records the profile
in `build-info.json`. A script-generated Cargo `RUSTC_WRAPPER`
enables `--cfg reth_sparse_trie_coverage` and appends
`-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=3
-Cllvm-args=-sanitizer-coverage-trace-pc-guard` only when compiling
`reth_trie_sparse`, preserving all original compiler arguments. Other crates,
including the adapter and core driver, stay uninstrumented to avoid coverage
self-observation. Existing compiler wrappers and external SanCov flags are
rejected. The target crate's optional `fuzz` Cargo feature disables expensive
structural assertions and orphan-node checks. This workload must run those checks,
so the wrapper rejects that feature and requires debug assertions on the target
crate. The `fuzz` build profile does not enable the similarly named Cargo feature. That checker detects allocated trie nodes that are no longer
reachable from the root. The independent `reth_sparse_trie_coverage` cfg enables the
sparse-trie-specific coverage buckets. These classify node
neighborhoods by depth, target nibble and neighboring branch pattern: 28672
combinations plus two proof-state controls in each backend. LTO stays disabled
so their distinct machine-code functions survive. Preparation checks all 28674
functions per backend in the linked library and records counts in `build-info.json`;
any missing bucket function in either backend fails preparation. These semantic target buckets are separate
from generic native-edge hit counts and the driver's operation-count ranges. The default target directory is `target/sparse-trie-differential-sancov`.
It retains the wrapper, library, native dependencies, source archive, hashes, toolchain versions and Git provenance
under `$RUN_DIR/execution/`. Build and run on the same Linux machine; copying
native libraries does not guarantee compatibility with another machine's runtime.

The generated session loads the retained library. `$RUN_DIR/fuzz-session.json` is a format-version-2 pointer to the real
session at `execution/fuzz-session.json`. That session imposes no worker cap, keeps
all 72 initial inputs (`max_seeds: 0`), checks every execution fully
(`finalize_every: 1`), and generates a fresh random-seeded input every 100
successful search executions. The 72 seeds are 0–63, 64, 127, 128, 255, 256, 65535, 18446744073709551614,
and 18446744073709551615. Search executions are mutations after the initial corpus
has run. The outer pointer lets Rift retain the entire execution bundle together.
When submitting through the host, select `contrib/sparse-trie-differential` from a Git source pinned
to a full commit SHA, let the host select execution concurrency from available cores, and set
the intended duration. Leave stop-on-first-objective disabled to collect
multiple differences. Preparation tests the native calling interface before
writing the session pointer. Each client is a separate process with sequential
native ABI calls. Each call also exercises trie-internal parallelism in its bounded pool;
multiple clients are not a substitute for this.

## Local validation

```sh
cargo test --locked --manifest-path contrib/sparse-trie-differential/Cargo.toml
python3 contrib/sparse-trie-differential/test_prepare.py
printf '{}\n' > /absolute/workload-config.json
RUN_DIR=/absolute/new-inputs ./contrib/sparse-trie-differential/prepare.sh \
  --workload-config /absolute/workload-config.json
python3 contrib/sparse-trie-differential/smoke.py \
  /absolute/new-inputs/execution/libworkload.so --all-seeds
```

The Python smoke loads the actual library through `ctypes`, checks the methods
in the Native calling interface section, checks that execution creates no output
files, and checks local workers exit after every execution (allowing up to one
second for Linux procfs to remove joined threads). Loading the library starts no threads.
It requires actual instrumented target coverage on both worker threads across the
corpus, rather than merely checking that threads were created. It also checks
callback counts, malformed-request errors, null handles and deterministic
mutation, registered native edge counters, input-sensitive coverage, and exact
semantic replay after other executions (excluding duration and native probe-count
noise). It executes four generated inputs by default, or every initial seed with `--all-seeds`,
plus one locally mutated input per operator and one deterministic replay.
Objectives are counted, not relabeled as calling-interface errors. A plain
`cargo build` is intentionally insufficient: execution rejects absent native
coverage instead of silently falling back to semantic counters.

## Native calling interface

Rift retains the loaded library and one native handle (opaque state pointer)
per worker. Per-input calls use C function pointers, not a subprocess transport.
A handle must not cross a fork or be shared between workers. The library exports
`rift_workload_create_v1`, `rift_workload_call_v1`, and
`rift_workload_destroy_v1` using C ABI version 1, independently of session version 2.
The caller supplies a reply callback receiving a pointer and byte length. Its bytes
are borrowed from the library and must be copied before the callback returns; the
callback must neither reenter the library nor unwind. Creation accepts exactly `{"args":{},"search":{"finalize_every":1}}`,
the runtime subset of the generated session. `finalize_every: 1` requires every
execution to run the full oracles; skipping correctness checks is unsupported.
Success returns a handle without invoking the callback; failure returns null and one UTF-8 diagnostic.
Calls accept `{"method":"METHOD","params":{...}}`, at most 65536 bytes.
They invoke the synchronous borrowed-buffer callback exactly once: status zero
means a JSON result; nonzero means a UTF-8 diagnostic. The methods below either prepare inputs or execute them. Input-planning calls
(`seed-inputs`, `fresh-input`, `mutation-operators`, `mutation-sites` and
`mutate-input`) require `args: {}`. `execute` runs an input; `finish` acknowledges
its retained result.

- `seed-inputs` returns the input count, or one input when given `index`.
  `fresh-input` deterministically derives an input from the host's random `seed`.
- `mutation-operators` and `mutation-sites` advertise actual typed action/update/
  key/value/prune-cutoff paths, not fixture-seed mutations. A site identifies one
  editable location as an array of field names and indices. `mutate-input` deterministically
  applies `action.insert/remove/duplicate/crossover/swap`,
  `update.insert/remove/duplicate/crossover`, `key.update`, `key.prefix`,
  `update.lifecycle` (toggle account metadata/deletion), `value.update`, or `prune.update` to the selected local site. Action/update
  crossover and `key.prefix` (nibble-prefix crossover) require both `donor` input text and
  `donor_path` (the selected location within that donor); other operators reject donors. Indices and action types are
  validated before mutation. Operator metadata omits fixed path selectors so Rift asks for the
  current capacity-checked sites. Byte-budget exhaustion safely returns the unchanged
  input instead of a planning error. Removal supports shrinking; unchanged rounds and the
  fixture seed remain intact. `key.prefix` crossover replaces a key prefix while keeping its suffix,
  while value mutations cover deletion and RLP integer boundaries. There are no
  database, sleep or artificial commit-boundary operators.
- `execute` runs all checks immediately and returns native `coverage` slot/count
  pairs, `coverage_size`, a build/layout `coverage_id`, `coverage_threads` (distinct
  threads that hit instrumented target code), semantic features and a
  verdict. Rift uses new counters or features to decide **corpus admission**:
  whether to keep the input for later mutation. The result is `pending: true`
  until Rift calls `finish` with `admitted` (that decision) and `n` (its execution
  number). `finish` returns the retained verdict without repeating execution.
  Other calls on that handle are rejected until `finish`, including after an
  objective. `pending: false` means no finish is required.
- Core-reported differences return objectives with status zero. Malformed requests
  and unexpected panics are workload errors, not trie differences. Rust unwinds
  are contained at exported boundaries; an unexpected call panic permanently
  poisons its handle, rejecting later calls. Native aborts, OOM, invalid C pointers and signals cannot
  be recovered by catching Rust unwinds.

Coverage is LLVM SanCov **native edge coverage** in `reth_trie_sparse`, not a
proxy derived from scenario names or seeds. Its compiler constructor calls
`__sanitizer_cov_trace_pc_guard_init` to assign stable slot IDs to static guards.
The exported `__sanitizer_cov_trace_pc_guard` callback increments adapter-owned
atomic 8-bit counters, so parallel target threads do not race on LLVM's plain
inline counters. The native ABI keeps library code and guards mapped until
process exit, including after worker fork. A process-local mutex serializes each
clear/run/read transaction across handles; native ABI calls remain sequential,
while joined Rayon tasks may run instrumented target code concurrently. The
adapter and callbacks are uninstrumented, preventing coverage self-observation.
Only machine code compiled in `reth_trie_sparse` receives native instrumentation;
generic functions instantiated in downstream crates are not covered by it.

The adapter clears all native counters immediately before calling the example's
`run_input(&input)` entrypoint (included as module `core`), then
returns sorted nonzero `(slot, raw 8-bit count)` pairs. Atomic counters wrap modulo
256; a wrapped zero is omitted. Slots follow guard registration order. The
`coverage_id` includes the SHA-256 of the actual instrumented target rlib and a
hash of the full guard layout, independent of ASLR or observed edges. Its `v2`
namespace distinguishes this atomic guard layout from the earlier inline-counter
builds.
Coverage is comparable within this exact retained build, not across rebuilds.
The target’s hash-table probing and parallel scheduling can vary raw native counts for the same input;
the smoke test reports this variation but requires identical semantic features
and oracle results. Native coverage is not claimed to be byte-identical on replay.
Semantic scenarios and logarithmic operation-count buckets remain separate
novelty features. Timing, raw seeds and hashes never become novelty features.
Differences retain the native edges observed before the failed oracle, even when
the core has no complete semantic report. Successful verdicts include the complete
core report. The plugin writes no execution artifacts and installs no
process-global logger or panic hook.
