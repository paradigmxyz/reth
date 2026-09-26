# Complete state trie: RocksDB versus MDBX

Measured on dev-brian on **2026-09-25**, using three profiling binaries built
from commit `32bac3f7d78d330d4f9757b241426551bb0b433e`. Only the backend feature
selection differs: default (legacy MDBX), `state-trie-db` (complete MDBX), or
`state-trie-rocksdb` (complete RocksDB).

## Results

RocksDB improves persistence and sustained replay throughput, but individual
payloads take longer to validate. Relative to complete MDBX, median server
validation increases **71.5%**, from **124.07 to 212.78 ms**. Mean execution
increases **84.9%**, from **120.08 to 222.01 ms**. Mean validation time outside
execution stays nearly unchanged: **14.07 versus 14.49 ms**.

All runs recorded **zero state-trie overlay computations**. Execution continues
to use the BundleState overlay; database misses select the configured backend.
The slowdown is inside BAL execution, which includes database reads, not a
return of the previous trie-overlay computation. These timings do not isolate
lookup, decompression, allocation, or concurrent proof costs.

Across all 600 blocks and the full shutdown flush, cumulative `save_blocks`
time falls **92.9%**, from **858.78 to 60.86 seconds per run**. Sustained replay
throughput rises from **0.785 to 3.248 Ggas/s**, or **4.14×**. Legacy MDBX reaches
**0.972 Ggas/s**. RocksDB has higher server validation latency while avoiding
the large persistence backpressure seen with both MDBX schemas.

Durations below are **milliseconds**. Each cell is **p50 / p90 / p99**. Block
metrics pool 1,650 samples per backend; proof metrics pool individual jobs.

| Metric | Legacy MDBX | Complete MDBX | Complete RocksDB |
| --- | ---: | ---: | ---: |
| Server payload validation | 125.88 / 149.74 / 409.62 | 124.07 / 149.15 / 388.15 | 212.78 / 297.28 / 713.87 |
| Client newPayload RPC | 151 / 180.10 / 41,986.43 | 149 / 178 / 50,631.09 | 238 / 332 / 742.20 |
| BAL execution | 112.92 / 133.14 / 374.75 | 111.03 / 132.39 / 349.69 | 199.15 / 278.01 / 700.61 |
| Post-execution state-root wait | 0.0035 / 0.0048 / 16.9052 | 0.0022 / 0.0038 / 8.4814 | 0.0023 / 0.0065 / 9.0687 |
| Account proof job | 0.887 / 2.884 / 11.288 | 0.553 / 2.238 / 9.549 | 1.639 / 5.658 / 16.164 |
| Storage proof job | 0.218 / 1.413 / 5.854 | 0.138 / 0.881 / 5.075 | 0.482 / 1.808 / 6.164 |

Mean remaining state-root wait is **0.719 / 0.292 / 0.372 ms**, respectively.
It stays small because proof and root work overlap execution. Mean account
proof-job duration is **1.449 / 1.062 / 2.645 ms**, and mean storage proof-job
duration is **0.605 / 0.433 / 0.858 ms**.

| Proof work after warmup | Legacy MDBX | Complete MDBX | Complete RocksDB |
| --- | ---: | ---: | ---: |
| Account jobs | 3,148,280 | 3,140,722 | 3,424,028 |
| Storage jobs | 3,446,409 | 3,432,505 | 3,734,090 |
| Mean account service sum/block (ms) | 2,764.49 | 2,021.40 | 5,489.34 |
| Mean storage service sum/block (ms) | 1,262.97 | 900.98 | 1,941.13 |

RocksDB dispatches about 9% more jobs in this replay. Worker service sums
include overlapping work and waiting; they are neither elapsed block time nor
CPU time. The proof and sparse-trie algorithms are shared between the two
complete-trie backends.

### Persistence

`save_blocks` durations are **seconds per batch**, including commit and BAL
flush. The primary window includes batches wholly after warmup that finish
before the final replay submission.

| Window | Backend | Batches | p50 | p90 | p99 | Mean |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Primary | Legacy MDBX | 42 | 42.52 | 77.69 | 85.08 | 39.86 |
| Primary | Complete MDBX | 42 | 55.64 | 81.06 | 100.75 | 49.74 |
| Primary | Complete RocksDB | 99 | 1.66 | 2.01 | 2.38 | 1.67 |
| All, including shutdown | Legacy MDBX | 54 | 42.52 | 81.68 | 98.59 | 41.00 |
| All, including shutdown | Complete MDBX | 54 | 55.64 | 82.20 | 100.71 | 47.71 |
| All, including shutdown | Complete RocksDB | 108 | 1.67 | 2.03 | 2.37 | 1.69 |

Batch sizes differ: primary batches average **32.93 / 34.52 / 16.00 appended
blocks**, respectively. The per-batch speedup overstates the improvement per
block. Primary save time divided by appended blocks is **1.211 / 1.441 /
0.104 seconds per block**. Each backend appends exactly 1,800 blocks across
its three complete runs, including shutdown:

| All persistence | Legacy MDBX | Complete MDBX | Complete RocksDB |
| --- | ---: | ---: | ---: |
| Mean cumulative save time/run (s) | 738.08 | 858.78 | 60.86 |
| Cumulative save time/appended block (s) | 1.230 | 1.431 | 0.101 |
| Mean steady replay throughput (Ggas/s) | 0.972 | 0.785 | 3.248 |

Persistence overlaps execution, so cumulative save duration must not be added
to replay wall time. The small batch sample count limits interpretation of
save-duration p99. Client RPC p99 includes engine backpressure; the server
validation timer excludes the preceding persistence waits.

### Repetitions

Each row scores the same 550 blocks after 50 warmup blocks. Replay time runs
from the 50th to the 600th completed submission, including forkchoice updates
and normal engine backpressure, excluding shutdown.

| Run | Validation p50 (ms) | p90 (ms) | p99 (ms) | Mean (ms) | Replay (s) | Ggas/s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| rocks-1 | 213.49 | 297.93 | 725.02 | 237.64 | 172.49 | 3.245 |
| old-1 | 125.61 | 149.77 | 381.02 | 135.93 | 559.83 | 1.000 |
| new-1 | 123.61 | 148.26 | 395.35 | 133.61 | 716.20 | 0.782 |
| new-2 | 123.97 | 149.24 | 376.13 | 134.55 | 716.72 | 0.781 |
| rocks-2 | 212.40 | 296.12 | 741.52 | 238.12 | 172.61 | 3.243 |
| old-2 | 126.20 | 150.37 | 401.11 | 136.56 | 570.25 | 0.982 |
| old-3 | 125.54 | 147.38 | 412.72 | 136.81 | 599.04 | 0.934 |
| new-3 | 124.38 | 148.91 | 386.98 | 134.28 | 706.79 | 0.792 |
| rocks-3 | 213.12 | 292.86 | 670.39 | 233.73 | 171.96 | 3.255 |

Including warmup, mean validation is **136.88 / 134.45 / 233.93 ms**, and mean
execution is **122.55 / 120.49 / 219.60 ms**.

## Execution slowdown: cache-size diagnostic

A follow-up replay changed only the shared RocksDB block cache from its
**128 MiB default to 4 GiB** (`--db.rocksdb-block-cache-size 4294967296`). Both
runs used the same binary and first 250 BAL blocks, with fresh recovery and
Linux page-cache clearing. The first 50 blocks are excluded below. Both were
profiled with `perf record -F 49 --call-graph dwarf,4096` during replay.

| Duration (ms) | RocksDB 128 MiB | RocksDB 4 GiB | Earlier complete MDBX, same block range |
| --- | ---: | ---: | ---: |
| Execution p50 | 183.08 | 120.46 | 109.81 |
| Execution mean | 193.57 | 133.43 | 120.25 |
| Execution p90 | 216.17 | 139.80 | 135.79 |
| Execution p99 | 579.86 | 522.03 | 323.76 |
| Payload validation p50 | 196.41 | 132.66 | 122.45 |
| Payload validation mean | 206.84 | 146.52 | 134.40 |

The larger cache reduces median execution **34.2%** and mean execution **31.1%**.
This demonstrates that the small shared cache accounts for much of the
slowdown in this range. The original defaults were retained when adding the
182 GB of complete trie tables. Those tables share the cache with existing
RocksDB column families; proof workers and execution database misses compete
for it. Execution still checks the BundleState overlay first. A complete-trie
database miss uses an exact RocksDB `Get`, with no trie iterator construction.

The original 600-block RocksDB runs recorded **789–792 GB of logical read bytes**
(`io.rchar`) and **39.3–39.4 million read syscalls**, versus **75.5–75.7 GB of
physical read bytes** (`io.read_bytes`). These process-wide counters include
all work, not just execution; logical reads must not be mistaken for disk
traffic. The CPU profile from this initial diagnostic missed the lazily created
BAL, prewarming, and proof threads, so it cannot attribute their costs. The
[subsequent execution-latency investigation](state-trie-rocksdb-latency.md)
corrects the profiling method and identifies repeated copying/decompression
and the cross-database snapshot lock as the main causes of the regression.

This is one diagnostic run per cache size, scoring 200 blocks each, rather than
a replacement for the three-by-600 benchmark above. The MDBX column pools the
same 200-block range from its earlier three runs without perf sampling. All
500 diagnostic payloads and both restart payloads validated, the roots matched
each other and the original runs, and the logs were error-free. Final recovery
restored the promoted baseline. No source code or default settings changed.

Raw profiles, timing results, commands, and logs are preserved under the main
artifact directory's `execution-diagnosis/`. The first wrapper required a
bookkeeping correction for perf's normal SIGINT exit after collecting all 250
blocks; its checkpoint and restart were subsequently verified. An invalid CLI
unit attempt never started the node and is excluded.

## Migration, storage, and promoted baseline

After `schelk recover`, migration rebuilt **2,618,677,390 nodes** in RocksDB
from the original hashed leaves. Rebuilding took **89 minutes**; final
compaction and verification took approximately **8 minutes**. The source
tables and complete MDBX tables retained their entry counts and allocation.

| State representation | Table storage (bytes) | Decimal GB |
| --- | ---: | ---: |
| Legacy four MDBX tables | 208,051,847,168 | 208.05 |
| Complete MDBX account/storage tables | 509,180,637,184 | 509.18 |
| Complete RocksDB account CF | 38,078,741,412 | 38.08 |
| Complete RocksDB storage CF | 143,740,489,468 | 143.74 |
| Complete RocksDB total | 181,819,230,880 | 181.82 |

RocksDB SST storage is **64.3% smaller** than complete MDBX and **12.6% smaller**
than legacy MDBX. MDBX figures are allocated table pages; RocksDB figures are
compacted SST bytes, excluding WAL and shared metadata. The whole RocksDB
directory occupies **181,968,535,552 bytes**, including pre-existing auxiliary
tables. All schemas are retained, so RocksDB adds allocation to this snapshot.

`schelk promote --yes` completed after migration and root verification. Final
recovery after all benchmarks restored the promoted snapshot, mounted at
`/schelk`, with Finish and partial-state-trie both **24,979,000**. Both migrated
backends and the legacy root agree:

`0xc60084882e9a560a2679b26dbec4a5c7a6f5ac27a02985776706b82e45d7e254`

Available space is **2,103,585,624,064 bytes = 2.10 TB = 1.913 TiB**. No replayed
state was promoted. The promoted superblock hash is
`a9a302a983bc993722a14a514b16eb4b6ac2ac464ffa773d290ffd1998371c72`.

## Correctness and limitations

All **5,400 measured payloads returned VALID**, all 600 roots match across all
nine runs, and the measured replay logs, including their shutdown flushes,
contain no errors. Every block used BAL execution and the sparse-trie root
strategy. Every run recorded zero trie-overlay computations, root-task
timeouts, and successful serial fallbacks. Each run persisted Finish
**24,979,600**, without partial-state-trie lag. The final measured root is:

`0x378e9efc76298c47ca401a2c9da04ce0bc38a08f5d1a69ff9b71b95084200012`

Each node was restarted and validated the next block from persisted state.
All nine block-601 submissions returned VALID, reached Finish **24,979,601**
without frontier lag, and produced the same root:

`0x3cdb49c6633f5397dd2c9f316bd6cf7bed719adf55c59a9728062cf48bc264bc`

**One unmeasured restart check had a subsequent pruning failure.** In `old-2`,
after the restart block was saved, the persistence service logged
`PrunerError ... Corruption: ZSTD Data corruption detected`. The process still
exited with status zero. This was outside the measured replay and after its
full shutdown; its performance samples are retained. The other eight restart
logs are error-free. The corruption cause is undetermined. The error is
preserved in `old-2/restart-node.jsonl.gz` and `manifest.json`; final recovery
discarded that replay state. Restart payload validation succeeded, but this
run was not an entirely error-free restart/shutdown test.

The supported scope remains forward happy-path validation. Reorgs, historical
state, serial fallback, pipeline sync, RPC, payload building, storage wipes,
and crash recovery are outside the PoC. MDBX/RocksDB commit synchronization
provides consistent live snapshots; it is not a cross-database crash-atomic
transaction.

## Method and configuration

The run order was RocksDB/legacy/complete, complete/RocksDB/legacy,
legacy/complete/RocksDB. Each run replayed the same first 600 records of
`/schelk/bench/txgen-big-blocks-mainnet-24979001-1000-1G.ndjson`, all containing
merged BALs. Gas ranges from 1,000,014,208 to 1,058,504,590 per block, averaging
1,017,547,908. Each run executes 9,083,552 transactions, combining 20,195 source
blocks into 600 synthetic blocks. Selected-record SHA-256:
`5a729f23dad32e014413ab6cb9a3f4a13c3c93bccd5c979201d00e1a77a55728`.

Before each run, recovery restored the promoted baseline, both migration
`--check` modes verified the persisted roots, and Linux page caches were
cleared. There were no peers, concurrent builds, analyses, or other replays
during measurement and shutdown flushing. Hardware is an AMD EPYC 4585PX,
16 cores / 32 threads, with 62 GiB RAM. Builds use the `profiling` profile;
exported binaries have debug sections removed, retaining executable code and
function symbols. Commands and hashes are in `build-info.json`.

All modes use persistence threshold **50**, memory block buffer **5**, state
masking **30**, and state-root task timeout **0s**. `bench send-blocks` uses
`--wait-for-persistence never`, one forkchoice update per block, and 500 ms
metric scrapes. Normal engine persistence backpressure still applies. JSON
logging includes identical proof, execution, root-wait, and persistence timing
events. Shutdown allows pending persistence to drain completely.

RocksDB uses existing defaults: a shared **128 MiB block cache**, **128 MiB
memtable per CF**, **4 GiB write-buffer budget**, six background jobs, LZ4
compression with Zstd at the bottom level, and synchronous WAL writes. No
backend-specific tuning was applied. This short replay does not establish
long-running compaction equilibrium or write amplification.

The first 50 blocks per run are excluded from primary statistics. Percentiles
use linear interpolation; repeated blocks are not independent unique workloads.
Payload validation is the server timer; RPC duration also includes earlier
engine waits and transport. Execution includes BAL execution, provider reads,
and rebuilding the BAL. State-root wait measures `state_root_job.finish` after
execution, not total root computation. Account proof jobs include waiting for
storage proofs; storage jobs measure calculator service duration. Job timers
exclude initial queue delay and overlap execution and other workers.

## Implementation validation and artifacts

Affected-package tests passed **354** with default features and **348** with
the RocksDB feature. Eight RocksDB tests were excluded: four historical/plain
state tests outside the PoC and four RPC tests whose injected reentrant commit
hook deadlocks against the snapshot-consistency lock. These tests pass in
default mode; exact names are in `validation.json`. The two migration tests,
all-feature workspace Clippy, nightly formatting, `zepter`, TOML linting, and
private documentation builds passed. New tests cover cursor/proof equivalence,
snapshot isolation, persistence masking, migration, and execution without
computing a trie overlay.

Artifacts are in `/home/ubuntu/state-trie-rocksdb-benches-20260925`:

- `results.json`, `comparison.md`, and per-run `blocks.csv`, `saves.json`, and
  `summary.json` contain samples and summaries.
- `run.py`, `analyze.py`, command JSON files, corpus/build metadata, compressed
  node logs and reports preserve reproduction inputs and raw results.
- `latency-distributions.png` and `.pdf` plot validation, execution, and root
  wait distributions.
- Migration, promotion, baseline checks, final disk space, and schelk status
  are preserved alongside `manifest.json` and `validation.json`.

This fresh same-commit comparison is separate from the earlier two-backend
results in `docs/state-trie-db-benchmarks.md`.
