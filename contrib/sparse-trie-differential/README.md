# Sparse trie differential workload

Rift is a coverage-guided fuzzer: it generates and mutates inputs, runs them,
and keeps inputs that expose new native code paths or semantic features (named
scenarios and operation-count buckets). This plugin is the shared library Rift
loads into each worker process to generate, mutate and execute inputs. Its comparison logic (the **core**) is the shared
example in `crates/trie/sparse/examples/sparse_trie_differential.rs`: it compares
`ArenaParallelSparseTrie` and `PersistentArenaParallelSparseTrie`, two sparse-trie
backends, against independent root and update checks called **oracles**. A detected
difference is an **objective**, returned to Rift with an `objective_signatures`
category and diagnostic details. Replaying the input seed reproduces the operation
sequence; successful reports include the seed, operation counts and scenarios.

Inputs are JSON text containing one unsigned 64-bit `seed`. Replaying an input
recreates the same operation sequence. The host also supplies randomness for
scheduling and mutation, but execution uses only the input's seed. Execution is
single-threaded and in memory: no database, output files, network or subprocesses.
Preparation and Rift's result management may use disk. The core disables parallel
work by setting all four parallelism thresholds on both backends to their integer
types' `MAX`; their exact field names are in the shared example.

## Prepare on the execution machine

Requirements: Linux, a C linker, Rust 1.96 or newer with LLVM SanCov support,
Cargo, Git, Python 3, `ldd`, and access to public Cargo and Git dependencies. The independent Cargo workspace
uses path dependencies into the supplied checkout, with a checked-in lockfile and
public dependency revisions. No private SDK, container upload or registry-hosted
workload library is required. Existing Cargo manifests and CI remain unchanged.

```sh
printf '{}\n' > /absolute/workload-config.json
RUN_DIR=/absolute/new-inputs ./contrib/sparse-trie-differential/prepare.sh \
  --workload-config /absolute/workload-config.json
```

The preparation configuration accepts only `{}`; runtime settings are generated
in the session described below. `RUN_DIR` must be outside the checkout. Preparation refuses to replace an existing
session. Optional `CARGO_BUILD_JOBS` (default 2) and `CARGO_TARGET_DIR` control
build resources, not execution concurrency. The script compiles the exact supplied
tree **on the execution machine**, using `--locked`. A generated `RUSTC_WRAPPER`
appends `-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=3
-Cllvm-args=-sanitizer-coverage-inline-8bit-counters` only when compiling
`reth_trie_sparse`, preserving all original compiler arguments. Other crates,
including the adapter and core driver, stay uninstrumented to avoid coverage
self-observation. Existing compiler wrappers and external SanCov flags are
rejected. The default target directory is `target/sparse-trie-differential-sancov`.
It retains the wrapper, library, native dependencies, source archive, hashes, toolchain versions and Git provenance
under `$RUN_DIR/execution/`. Build and run on the same Linux machine; copying
native libraries does not guarantee compatibility with another machine's runtime.

A **session** is the JSON configuration the host reads to load this retained
library. `$RUN_DIR/fuzz-session.json` is a format-version-2 pointer to the real
session at `execution/fuzz-session.json`. That session requests one worker, keeps
all 72 initial inputs (`max_seeds: 0`), checks every execution fully
(`finalize_every: 1`), and generates a fresh random-seeded input every 100
successful search executions. The 72 seeds are 0–63, 64, 127, 128, 255, 256, 65535, 18446744073709551614,
and 18446744073709551615. Search executions are mutations after the initial corpus
has run. The outer pointer lets Rift retain the entire execution bundle together.
When submitting through the host, select this directory from a Git source pinned
to a full commit SHA, request one execution worker, and set
the intended duration. Leave stop-on-first-objective disabled to collect
multiple differences. Preparation tests the native calling interface before
writing the session pointer.

## Local validation

```sh
cargo test --locked --manifest-path contrib/sparse-trie-differential/Cargo.toml
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
noise). It executes four inputs by default, or every initial seed with `--all-seeds`, plus one deterministic replay.
Objectives are counted, not relabeled as calling-interface errors. A plain
`cargo build` is intentionally insufficient: execution rejects absent native
coverage instead of silently falling back to semantic counters.

## Native calling interface

Rift loads the library before forking workers, then creates one handle per worker.
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
- `mutation-operators` and `mutation-sites` describe seed bit-flip, addition and
  replacement. `mutate-input` applies one using `path: ["seed"]`; it always changes
  the seed and returns JSON text. Because operations are generated from the seed,
  Rift's input minimizer cannot delete individual trie operations from this
  seed-only representation.
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
  poisons its handle. Native aborts, OOM, invalid C pointers and signals cannot
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
`core::run(seed)` entrypoint, then
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
