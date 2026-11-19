# Sparse trie differential workload

Rift is an external coverage-guided fuzzer (the **host**). It generates and mutates
inputs, runs them, and keeps inputs that expose new native code paths or semantic
features: completed scenario names and logarithmic operation-count ranges. This
plugin is its native adapter, loaded once into each **worker** (also called a
client): a separate process that executes inputs sequentially. Its comparison logic (the **core**) is the shared
example in `crates/trie/sparse/examples/sparse_trie_differential.rs`: it compares
`ArenaParallelSparseTrie` and `PersistentArenaParallelSparseTrie`, two sparse-trie
backends, against independent checks called **oracles**. These rebuild expected
roots from an ordered leaf model and compare retained updates with a separate trie. A detected
difference is an **objective**, returned to Rift with an `objective_signatures`
category and diagnostic details. Replaying the complete input reproduces the operation
sequence; successful reports include the fixture seed, operation counts and scenarios.

Inputs are version-2 JSON text: `version`, an unsigned 64-bit `fixture_seed`,
and `rounds` containing exactly sixteen ordered action lists. The fixture seed
generates a starting trie with densely branching and long shared-prefix paths,
plus fixed checks of the sparse-trie API; explicit actions
are never regenerated during execution. Each round accepts up to eight actions:
`update_batch` (up to four full 32-byte key/value changes, zero values delete),
`reveal` (supply a model-derived proof for a key), `prune` (blind nodes older than
a numbered update epoch), `snapshot` and `clone` (copy and check the root), and
`tracking_reset` (clear the retained-update accumulator).
Repeated keys in a batch use its last value. Empty batches and action lists are
valid shrinking results; mandatory root/update, proof-retry and snapshot-isolation
checks still run. Inputs are limited to 12288 bytes, leaving room for escaped
input strings, including a second input used as a crossover **donor**, in the
65536-byte native request. Old seed-only inputs
are rejected. The host uses randomness to generate or mutate inputs, but its execution seed
never changes an already serialized action tape.
Execution is single-threaded and in memory: no database, output files, network or
subprocesses. Preparation and Rift's result management may use disk. The core
disables parallel work by setting all four parallelism thresholds on both backends
to their integer types' `MAX`; their exact field names are in the shared example.

## Prepare on the execution machine

Requirements: Linux, a C linker, Rust 1.96 or newer with LLVM SanCov support,
Cargo, Git, Python 3, `ldd`, GNU `nm`, and access to public Cargo and Git dependencies. The independent Cargo workspace
uses path dependencies into the supplied checkout, with a checked-in lockfile and
public dependency revisions. No private SDK, container upload or registry-hosted
workload library is required. CI remains unchanged.

```sh
printf '{}\n' > /absolute/workload-config.json
RUN_DIR=/absolute/new-inputs ./contrib/sparse-trie-differential/prepare.sh \
  --workload-config /absolute/workload-config.json
```

The preparation configuration accepts only `{}`. The script generates a
**session**: JSON settings telling Rift which library to load and how to search. `RUN_DIR` must be outside the checkout. Preparation refuses to replace an existing
session. Optional `CARGO_BUILD_JOBS` (default 2) and `CARGO_TARGET_DIR` control
build resources, not execution concurrency. The script compiles the exact supplied
tree **on the execution machine**, using `--locked`. A script-generated Cargo `RUSTC_WRAPPER`
enables `--cfg reth_sparse_trie_coverage` and appends
`-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=3
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters` only when compiling
`reth_trie_sparse`, preserving all original compiler arguments. Other crates,
including the adapter and core driver, stay uninstrumented to avoid coverage
self-observation. Existing compiler wrappers and external SanCov flags are
rejected. The `reth_trie_sparse` dependency also enables its `fuzz` feature; both switches
are required for its sparse-trie-specific coverage buckets. These classify node
neighborhoods by depth, target nibble and neighboring branch pattern: 28672
combinations plus two proof-state controls in each backend. LTO stays disabled
so their distinct machine-code functions survive. Preparation checks all 28674
functions per backend in the linked library and records counts in `build-info.json`;
a missing group fails preparation. These semantic target buckets are separate
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
When submitting through the host, select this directory from a Git source pinned
to a full commit SHA, request one execution worker per available CPU, and set
the intended duration. Leave stop-on-first-objective disabled to collect
multiple differences. Preparation tests the native calling interface before
writing the session pointer. Each client is a separate process with sequential
native ABI calls; using multiple clients does not enable trie-internal threads.

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
below, and verifies that execution creates neither output files nor extra threads.
It checks callback counts, malformed-request errors, null handles and deterministic
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
means a JSON result; nonzero means a UTF-8 diagnostic. Planning calls (all methods
except `execute` and `finish`) require `args: {}` in their parameters.

- `seed-inputs` returns the input count, or one input when given `index`.
  `fresh-input` deterministically derives an input from the host's random `seed`.
- `mutation-operators` and `mutation-sites` advertise actual typed action/update/
  key/value/cutoff paths, not fixture-seed mutations. `mutate-input` deterministically
  applies `action.insert/remove/duplicate/crossover/swap`,
  `update.insert/remove/duplicate/crossover`, `key.update`, `key.prefix`,
  `value.update`, or `prune.update` to the selected local site. Action/update
  crossover and nibble-prefix crossover require both `donor` input text and
  `donor_path`; other operators reject donors. Indices and action types are
  validated before mutation. Metadata omits static selectors so Rift uses the
  capacity-checked native sites. Byte-budget exhaustion safely returns the unchanged
  input instead of a planning error. Removal supports shrinking; unchanged rounds and the
  fixture seed remain intact. Prefix crossover keeps the destination suffix,
  while value mutations cover deletion and RLP integer boundaries. There are no
  database, sleep, account or artificial commit-boundary operators.
- `execute` runs all checks immediately and returns native `coverage` slot/count
  pairs, `coverage_size`, a build/layout `coverage_id`, semantic features and a
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
proxy derived from scenario names or seeds. Its compiler constructor calls the
exported `__sanitizer_cov_8bit_counters_init` to register each writable static
counter span. The native ABI keeps library code and these spans mapped until
process exit, including after worker fork. A process-local mutex serializes each
clear/run/read transaction across handles; execution calls must remain sequential
and target code must not run on other threads. Raw pointers are accessed only
before and after the core; no Rust counter references survive target execution.
Neither registration nor measurement spawns threads or writes files. The
unmodified target crate still contains generic functions instantiated downstream;
only machine code compiled in `reth_trie_sparse` receives native instrumentation.

The adapter clears all native counters immediately before calling the example's
`core::run_input(&input)` entrypoint, then
returns sorted nonzero `(slot, raw 8-bit count)` pairs. LLVM counters wrap modulo
256; a wrapped zero is omitted. Slots concatenate spans in constructor order;
the coverage identifier includes the SHA-256 of the actual instrumented target
rlib and a hash of the full span layout, independent of ASLR or observed edges.
Coverage is comparable within this exact retained build, not across rebuilds.
Randomized hash-table probing can vary some raw native counts for the same input;
the smoke test reports this variation but requires identical semantic features
and oracle results. Native coverage is not claimed to be byte-identical on replay.
Semantic scenarios and logarithmic operation-count buckets remain separate
novelty features. Timing, raw seeds and hashes never become novelty features.
Differences retain the native edges observed before the failed oracle, even when
the core has no complete semantic report. Successful verdicts include the complete
core report. The plugin writes no execution artifacts and installs no
process-global logger or panic hook.
