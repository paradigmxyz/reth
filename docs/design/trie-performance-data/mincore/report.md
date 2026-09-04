# MinCore probe suppression experiment: rejected

Disabling the supported `MDBX_USE_MINCORE` build option slows warm persistence. Durable transaction medians regress 5.35% with reopened mappings and 5.01% with persistent mappings; write-phase medians regress 31.91% and 33.52%. Cold performance is neutral. No production or vendored source was changed, and this setting should not proceed to Derek.

## Results

Two ABBA rounds produce four fresh processes per variant, each with 12 measured rounds and four warmups per case. Values below are medians and inclusive quartiles of process-level sample medians, not confidence intervals. Positive changes mean slower.

| Case | Default, durable ms [IQR] | MinCore disabled, durable ms [IQR] | Durable change | Write phase, default → disabled ms | Write change |
| --- | ---: | ---: | ---: | ---: | ---: |
| warm | 59.020 [58.173, 59.385] | 62.180 [61.178, 63.411] | +5.35% | 13.735 → 18.118 | +31.91% |
| warm_open | 58.190 [56.930, 58.934] | 61.107 [59.757, 62.226] | +5.01% | 12.838 → 17.141 | +33.52% |
| cold | 169.429 [168.315, 170.211] | 168.434 [166.684, 170.067] | -0.59% | 124.418 → 124.055 | -0.29% |

All four candidate process medians for each warm write phase exceed all four default process medians. The durable warm process quartiles also separate. Commit medians move about 1.2–1.4 ms lower in the candidate, with overlapping quartiles, partially offsetting the roughly 4.3 ms additional write cost. Medians of separate phases need not sum to the median of their total. No additional repeats are justified for a setting that clearly regresses both warm write paths.

## Mechanism and correctness

The default executes 141 MDBX mincore calls per measured transaction; the candidate executes zero. Warm default transactions perform no prefault operations, while the candidate performs 8,924. In cold transactions the default median is 8,918.5 prefault operations and the candidate remains 8,924. These are MDBX operation counters: a prefault operation can be pwrite or a batched pwritev, so the field is not a byte count or a universal page count. A mincore call can inspect 64 pages, so ratios of these counters are not residency probabilities.

Minor-fault medians are unchanged: 9,482 with reopened warm mappings and 8,924 with persistent warm mappings. Both warm cases have zero major faults. Cold major-fault medians remain exactly 8,918.5 in both variants, while minor faults remain 8,924. The preserved prefault path therefore avoids the earlier prefault-off experiment's extra cold destination reads, but redundant warm writes outweigh the removed probe work.

Of the 96 measured cold samples, 94 had no resident file pages before opening; the other two had only 4 of 18,432 pages resident (0.022%). The harness evicts only the disposable file after closing mappings and verifies residency with an independent Python/libc mincore call; disabling the MDBX compile option does not disable this verification.

All 288 measured rounds and 96 warmup rounds matched complete account/storage maps computed independently from the generated updates. Each process/case finishes with an aborted account mutation and physical database reopen, giving 24 final rollback/reopen checks. Every database open asserts FIFO reclamation, WRITEMAP, disabled readahead, Durable sync flags, and the enabled runtime prefault option. Both executables report their actual `mdbx_build.options`; the runner checks macro 1/0, positive baseline probe counts, and zero candidate probe counts. Twelve prior smoke rows passed the same checks and their timings are excluded. These are commit/reopen tests, not power-loss simulations.

## Scope and build provenance

The diagnostic is based on `33c491663d96c545e49e8af4cb6edc233132f0ce`. Only a benchmark file and Cargo benchmark registration were added in `/tmp/reth-mincore-diagnostic-33`; the production code and vendored MDBX are unchanged. It reuses the prior prefault/LIFO fixtures but compares only default FIFO, WRITEMAP, and enabled prefault. The sole build difference is `CFLAGS=''` versus `CFLAGS='-DMDBX_USE_MINCORE=0'`. Actual C command lines are preserved in `mdbx-build-default.log` and `mdbx-build-disabled.log`.

Both binaries use Rust 1.98.1, profiling, `-C target-cpu=native`, ASM Keccak and the global Keccak cache, with separate fresh target directories. The identical diagnostic source hash, compiler versions, flags and binary hashes are in `provenance.json`. Timing ran on CPU 1 of an AMD EPYC 4585PX after all local builds/tests/docs and the Derek raw-data audit completed. Other agents held heavy CPU and I/O work through the measurement slot.

The fixture contains 32,768 account and 65,536 storage branch records and replaces 12,288 records per transaction. Four update rounds establish reclaimed-page reuse. Setup, input/map allocation, opening, residency checks and snapshots are untimed; the reported durable interval includes beginning a transaction, writing records, and durable commit. Warm means file-cache warm with mappings reopened per sample. `warm_open` retains mappings and reads the entire live snapshot after each round; it does not model selective cache use. Cold excludes the open/create-tables work equally in both variants, after residency is verified before opening. This is a small synthetic database and does not measure full-node memory pressure or real-block throughput.

The initial profile supported investigating expensive residency probes, but removing a hot function alone was insufficient evidence of a gain: replacement work matters. The read-only analysis and exact activation/readahead conditions are preserved in `feasibility.md`.

## Reproduction

Apply `diagnostic-harness.patch` to an isolated checkout of the base commit, then build the identical harness twice:

```sh
CARGO_TARGET_DIR=/tmp/mincore-default-target CFLAGS='' \
  RUSTFLAGS='-C target-cpu=native' CARGO_BUILD_JOBS=4 CC_ENABLE_DEBUG_OUTPUT=1 \
  cargo +1.98.1 bench --locked --profile profiling -p reth-trie-db \
  --bench mincore_probe --no-run \
  --features alloy-primitives/asm-keccak,alloy-primitives/keccak-cache-global

CARGO_TARGET_DIR=/tmp/mincore-disabled-target CFLAGS='-DMDBX_USE_MINCORE=0' \
  RUSTFLAGS='-C target-cpu=native' CARGO_BUILD_JOBS=4 CC_ENABLE_DEBUG_OUTPUT=1 \
  cargo +1.98.1 bench --locked --profile profiling -p reth-trie-db \
  --bench mincore_probe --no-run \
  --features alloy-primitives/asm-keccak,alloy-primitives/keccak-cache-global
```

Copy the produced executables, then run on a quiet machine using a new output directory:

```sh
python3 run.py /path/to/default-binary /path/to/disabled-binary \
  --output /tmp/mincore-repeat --samples 12 --rounds 2 --cpu 1
```

`results/` contains all raw per-process CSVs, build-option stderr, per-process medians and summary tables. Frozen executables, compile logs, diagnostic patch, runner and smoke logs are retained alongside this report.
