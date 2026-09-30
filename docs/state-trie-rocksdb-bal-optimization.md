# Overlapping BAL validation with the complete RocksDB trie

Measured on dev-brian, 2026-09-29–30. This records the ongoing complete-trie
RocksDB optimization. Both backends retain early signature recovery and the
4 GiB execution cache with code/storage/account weights 8/82/10.

## Latest 999-block results

The selected build beat the faster extended legacy reference twice, and a fresh
legacy control confirmed the result. Each run replayed the same 999 approximately
1 Ggas BAL blocks, excluding the first 50 from latency statistics. All save
durations, including warmup and shutdown drain, are counted.

| Metric | Legacy MDBX reference | Fresh legacy MDBX | Complete RocksDB | Exact RocksDB repeat |
| --- | ---: | ---: | ---: | ---: |
| Payload p50 / p90 / p99, ms | 103.074 / 126.976 / 356.588 | 103.427 / 125.073 / 344.922 | 102.794 / 159.688 / 337.706 | 102.367 / 153.874 / 298.553 |
| Payload mean, ms | 111.809 | 111.409 | 115.500 | 114.293 |
| Execution p50 / p90 / p99, ms | 89.171 / 106.708 / 317.940 | 89.537 / 103.607 / 318.379 | 90.592 / 125.614 / 195.297 | 90.634 / 128.631 / 195.612 |
| State-root wait p50 / p90 / p99, ms | 0.004 / 15.343 / 37.857 | 0.006 / 15.215 / 37.630 | 0.002 / 9.829 / 201.640 | 0.002 / 7.915 / 162.969 |
| Account proof job p50 / p90 / p99, ms | 12.594 / 36.702 / 52.264 | 12.684 / 37.170 / 53.412 | 12.641 / 23.606 / 41.132 | 12.189 / 22.930 / 39.083 |
| Storage proof job p50 / p90 / p99, ms | 0.190 / 3.678 / 18.055 | 0.192 / 3.730 / 19.001 | 0.214 / 3.422 / 17.905 | 0.206 / 3.356 / 17.960 |
| All save_blocks, seconds | 935.705 | 991.254 | 46.075 | 46.163 |

Against the faster legacy reference, the payload median advantage is small:
0.27–0.69%. Total persistence time is 20.3× lower. Payload p90 and mean, execution
p50, and state-root wait p99 are still worse; this is a measured median
improvement, not a win at every percentile. Proof-job populations differ
between schemas, so per-job percentiles do not measure identical units of work.
All 999 roots and restart block 1000 match across all four runs, and all 949
scored blocks overlap proof work with execution in each run. Fallback, timeout,
and execution's full state-trie-overlay construction counters remain zero.

## Matched 600-block results

Each run replays the same 600 approximately 1 Ggas BigBlockData blocks with BALs.
Latency statistics exclude the first 50 blocks. Entries below are p50/p90/p99
in milliseconds; persistence totals include **all** saves, including warmup
and shutdown drain.

| Metric | Improved legacy MDBX | Complete RocksDB | Exact RocksDB repeat |
| --- | ---: | ---: | ---: |
| Payload validation | 105.017 / 125.361 / 328.281 | 104.205 / 155.765 / 245.105 | 103.281 / 156.453 / 292.373 |
| Execution | 90.792 / 106.277 / 307.483 | 91.548 / 126.481 / 191.879 | 90.842 / 125.866 / 201.260 |
| State-root wait | 0.003 / 14.284 / 37.934 | 0.002 / 12.970 / 117.660 | 0.002 / 10.083 / 168.131 |
| All save_blocks, seconds | 321.739 | 28.475 | 28.610 |

Both RocksDB medians beat the fresh legacy control, with approximately 11.2×
faster persistence. Payload p90 remains worse; this is not an improvement at
every percentile. All 600 roots match, restart block 601 matches, and every
one of the 550 scored blocks overlaps execution with proof work. Fallback,
timeout, and full state-trie overlay counters remain zero.

The fresh legacy control is important: 12.4 GB of inactive benchmark binaries
and artifacts were moved out of tmpfs before these measurements, preserving
and verifying their contents. The older legacy save total of 941.57 seconds
was measured under different memory pressure and is **not** the denominator
for the claimed persistence speedup.

## Extended comparison and subsequent experiments

The same binaries were then tested over 999 blocks plus restart block 1000,
scoring blocks 51–999. All roots match, all 949 scored blocks overlap proofs
with execution, and both restart checks pass.

| Metric | Improved legacy MDBX | Complete RocksDB, 12 GiB |
| --- | ---: | ---: |
| Payload p50 / p90 / p99, ms | 103.074 / 126.976 / 356.588 | 105.703 / 162.692 / 256.917 |
| Execution p50 / p90 / p99, ms | 89.171 / 106.708 / 317.940 | 93.268 / 133.174 / 206.157 |
| All save_blocks, seconds | 935.705 | 49.887 |

The first 600 blocks of this legacy run had a 103.102 ms payload median,
showing that the earlier narrow wins against 105.017 ms were not conclusive.
The 12 GiB RocksDB configuration does not beat the fresh extended control.
Persistence remains 18.8× faster. Subsequent runs use the same 999 blocks and
checks:

| Complete RocksDB variant | Payload p50 / p90 / p99, ms | Execution p50, ms | All saves, seconds |
| --- | ---: | ---: | ---: |
| Metadata outside cache, 16 GiB data cache | 107.104 / 163.529 / 297.571 | 93.431 | 50.493 |
| 12 GiB, leveled compaction trigger 8 | 105.382 / 159.155 / 252.777 | 93.110 | 48.821 |
| Universal compaction, trigger 4 | 104.969 / 151.801 / 248.130 | 92.633 | 49.180 |
| Universal compaction, trigger 8 | 104.548 / 155.487 / 287.964 | 91.969 | 48.756 |
| Same, without internal statistics counters | 103.547 / 153.357 / 278.989 | 91.119 | 48.126 |

The statistics-free candidate was still 0.473 ms behind legacy MDBX's payload median,
with 19.4× faster persistence. The 16 GiB cache increased peak RSS to 41.6 GB
and reduced available host memory to 4.5 GB; the 12 GiB setting remains selected.
Raising the universal-compaction trigger halved new-table compaction CPU from
101.5 to 49.0 seconds, while increasing proof-worker CPU somewhat. Disabling
internal tickers reduced account/storage proof CPU from 85.4/206.0 to
84.6/203.4 ms per block. These CPU figures include all 999 blocks before drain.
Reducing global Rayon from 32 to 16 threads lowered its CPU time from 577.2 to
430.1 ms per block. Payload p50 only fell to 103.249 ms, with worse p90/p99 of
161.291/334.412 ms; total saves fell to 46.848 seconds. The next experiment
restored 32 threads and skipped legacy branch-mask derivation when only complete
node updates were retained. It passed all 195 sparse-trie tests and full replay,
but payload p50 regressed to 104.628 ms (p90/p99 149.254/244.400 ms), with 48.498
seconds of saves. Its small CPU reduction did not justify retaining it; the
change was reverted.

Further isolated experiments also missed the legacy median:

| Experiment | Payload p50 / p90 / p99, ms | All saves, seconds |
| --- | ---: | ---: |
| Host-targeted native RocksDB compilation | 104.181 / 149.897 / 248.021 | 47.955 |
| 14-bit Bloom filters, including the migrated snapshot | 104.235 / 154.811 / 252.884 | 49.071 |
| 10 GiB data cache | 104.267 / 155.281 / 275.490 | 47.821 |
| Uncompressed new SSTs and compaction outputs | 104.102 / 154.525 / 226.990 | 47.541 |
| Skip block-cache insertion for bulk prewarming | 104.871 / 152.383 / 276.841 | 48.352 |
| Contiguous proof-node collection | 103.931 / 147.998 / 240.600 | 48.237 |
| Worker-local decoded branch cache | 104.953 / 153.776 / 245.431 | 49.087 |
| 32 account-proof workers, unchanged best binary | 104.473 / 149.377 / 253.444 | 48.349 |
| 24 global Rayon threads, unchanged best binary | 103.760 / 150.894 / 273.489 | 47.910 |
| Larger storage jobs first, 16 Rayon threads | 103.525 / 151.732 / 322.918 | 47.509 |

The 10 GiB cache reduced physical reads from 103.02 to 97.50 GB but increased
logical reads and did not improve latency. Larger Bloom filters did not reduce
physical reads. Disabling compression reduced native background CPU from 59.11
to 48.72 ms per block and state-root wait p99 from 154.93 to 60.10 ms, but missed
the payload-median target. Suppressing block-cache insertion for bulk prewarming
reduced prewarm CPU from 121.34 to 118.16 ms per block, but increased storage-proof
CPU from 203.37 to 209.74 ms and regressed execution. These changes were reverted.
Contiguous proof-node collection reduced account/storage proof CPU from
84.62/203.37 to 82.99/200.76 ms per block but also missed the payload-median
target; it was reverted. A bounded per-worker decoded branch cache across
successive cursors hit only 3.09% of reads and increased account/storage proof
CPU to 88.91/212.06 ms per block. Snapshot, database, cursor, and secondary
isolation tests passed, but the cache was reverted after the latency regression.
Increasing account-proof concurrency from 16 to 32 workers reduced state-root
wait mean/p99 from 5.84/154.93 to 4.31/90.31 ms, but execution and payload medians
regressed. Sixteen account workers remain selected. The 24-thread global Rayon
run also missed the median target. Dispatching larger independent storage jobs
first, using the lowest-median 16-thread Rayon control, was slower than that
control's 103.249 ms and was reverted.

A separate diagnostic enabled scheduler accounting between blocks 50 and 999,
then restored the original disabled setting. Account workers averaged 83.52 ms
CPU and 389.51 ms runnable-queue wait per block; storage workers averaged
206.25 ms CPU and 1,136.64 ms queue wait. These are sums across workers, not block
latency. The counters include worker activity outside timed proof calls, so their
difference from proof-job durations cannot be labeled exact I/O wait. The run is
diagnostic and is excluded from candidate latency comparisons.

Compiling RocksDB's optional performance and I/O context instrumentation out
with `-DNPERF_CONTEXT -DNIOSTATS_CONTEXT`, using the unchanged statistics-free
Rust implementation and 16 Rayon threads, produced a first payload p50 of
102.794 ms (p90/p99 159.688/337.706 ms), execution p50 of 90.592 ms, and
46.075 seconds of all saves. This is 0.280 ms below the extended legacy median
and 20.3× lower total save time. Account/storage proof CPU fell from
85.04/202.25 to 82.43/195.13 ms per block compared with the same Rust implementation and
16-thread setting; the repeat measured 81.78/193.90 ms. These are worker CPU
sums across all 999 blocks before drain, not wall-clock proof duration. An exact repeat reached 102.367 ms with 46.163 seconds of saves. The fresh
legacy control measured 103.427 ms and 991.254 seconds, confirming both candidate
runs beat it as well as the faster 103.074 ms reference. Application timing
metrics remain enabled; native PerfContext and IOStatsContext diagnostics are
unavailable in this build.

## Retained implementation

- Extract complete-trie updates in parallel across account and storage tries.
  Avoid unused legacy updates, empty-map drains, and copying proof children.
- Accumulate complete-trie updates in contiguous vectors, preserve the newest
  value when deduplicating, and store short encoded values inline.
- Merge masked updates in parallel, including disjoint first-nibble account
  ranges and independent storage addresses.
- Dispatch account and storage proofs independently; complete account leaves
  already contain storage roots. A shared completion marker follows all results.
- For present keys, read the exact complete leaf and then its ancestors. For
  absent keys, descend from the requested subtree to an absence witness.
  Neither successor coverage nor reuse of an earlier target's ancestor chain
  is enabled.
- Stage sorted account/storage SSTs in parallel, warm their OS cache pages,
  and ingest with snapshot consistency before publishing the MDBX checkpoint.
  Existing snapshots retain their prior view. The two new column families use
  universal compaction with an eight-run trigger and 256 MiB target files.
- Use RocksDB HyperClockCache for this feature. Keep the two complete-trie
  tables' index/filter blocks in table readers, outside the data block cache.
  Legacy trie column families retain their previous metadata-cache policy.
- Disable native statistics tickers for the new RocksDB feature; database
  properties and application metrics remain available.
- Use 64 BAL prewarm workers for complete RocksDB state; honor the standard
  RAYON_NUM_THREADS setting. The default global thread count stays unchanged.

Against the otherwise identical preceding HyperClock/SST configuration,
keeping metadata outside the cache reduced proof CPU by approximately 7–8%
and prewarm CPU by about 10%. These CPU figures cover all 600 blocks before
shutdown drain, rather than the 550-block latency window. Peak node RSS was
about 32 GiB, with at least 11 GiB available host memory in both runs.

The account-memtable/storage-SST alternative reached 104.25 ms payload p50 but
required 35.84 seconds of saves, only 9.0× faster than the control, and was
reverted. MultiGet proof experiments, mmap reads, lower background priority,
earlier compaction, and larger shared data caches did not meet both targets.

## Reproduction and evidence

Build reth-bb with the profiling profile and state-trie-rocksdb feature.
The measured configuration uses a **12 GiB data cache**, 16 account proof
workers, 64 storage proof workers, 64 prewarm workers, and proof chunks of 80.
Metadata is outside the data-cache budget; the configured cache size is not a
limit on total RocksDB memory. The native-instrumentation-free candidate uses
16 global Rayon threads (`RAYON_NUM_THREADS=16`); BAL execution and signature
recovery pools each retain 32 workers on this host.

Persistence threshold/buffer/masking remain 50/5/30. RPC block, receipt, and BAL
caches hold 16 entries each. The state-root timeout is disabled. The sender uses
`--wait-for-persistence never`. CPU affinities and thread priorities match the
[previous host settings](state-trie-rocksdb-selected-baseline.md#runtime-settings).
Native background priority remains normal.

The trie SSTs are rewritten to the existing 4 KiB/LZ4 layout after each recover.
No snapshot promotion is performed. Migration, launch, complete node arguments,
thread audits, raw logs, metrics, root checks, source patches, and immutable
binaries are preserved under:

```text
/home/ubuntu/state-trie-bal-opt-20260929/
```

The selected binary is `reth-bb-u8noperf`, SHA-256:

```text
39af98d4b29a954e3bdb0224dcf25d5dcdaaaa23efe7f8587a760cb52ed449fe
```

The native build recipe and linked-library audit are in `native-noperf/`.
The library was compiled separately using `cargo +stable build --locked
--profile profiling -p librocksdb-sys`, a fresh `CARGO_TARGET_DIR`,
`ROCKSDB_STATIC=1`, `CXXFLAGS="-DNPERF_CONTEXT -DNIOSTATS_CONTEXT"`, and
`RUSTFLAGS="-C target-cpu=native"`. No C++ `-march=native` flag was used.
The node was then built with `cargo +stable build --profile profiling
-p reth-bb --features state-trie-rocksdb`, `ROCKSDB_STATIC=1`, and
`ROCKSDB_LIB_DIR` pointing to that library, without an additional Rust flags
override. Both builds used `CARGO_BUILD_JOBS=8`. Checksums and the complete
commands are preserved in `build-u8noperf.py` and `native-noperf/recipe.json`.
This is a build-time configuration; a normal build without these C++ flags
still includes the optional native context instrumentation.

The final 999-block result directories are:

```text
legacymdbx-hostcleanlong-rpcsmall-priority-isolated-vcache-proofside-cpupriority-chunk80-999
legacymdbx-finalpair-rpcsmall-priority-isolated-vcache-proofside-cpupriority-chunk80-999
newrocks-u8noperf-acct16-pref64-ray16-twelve-rpcsmall-priority-isolated-vcache-proofside-cpupriority-chunk80-999
newrocks-u8noperfrepeat-acct16-pref64-ray16-twelve-rpcsmall-priority-isolated-vcache-proofside-cpupriority-chunk80-999
```

The three 600-block result directories are:

```text
legacymdbx-hostclean-rpcsmall-priority-isolated-vcache-proofside-cpupriority-chunk80-600
newrocks-clockmeta-acct16-pref64-twelve-rpcsmall-priority-isolated-vcache-proofside-cpupriority-chunk80-600
newrocks-clockmetarepeat-acct16-pref64-twelve-rpcsmall-priority-isolated-vcache-proofside-cpupriority-chunk80-600
```

Each directory's `summary.json`, `blocks.csv`, `saves.json`,
`proof-execution-overlap.json`, and `node-command.json` contains the detailed
evidence. `CONTINUATION.md` records the complete experiment sequence.

## Validation

The retained implementation passed 364 focused trie/common/provider tests,
including randomized sparse-trie updates, proof equivalence, masked persistence,
SST snapshot isolation, aborted staging, and process-exit/reopen durability.
The final native-library build passed the 12 relevant proof/provider/completion
tests again. Legacy proof-worker and engine partial-completion tests also passed.
Workspace all-features Clippy (libraries, examples, tests, and benches), nightly
format checks, dependency linting, and affected-crate private documentation
checks passed. The final Rust diff matches the tested statistics-free candidate;
subsequent rejected experiments were restored before the native-library trial.
