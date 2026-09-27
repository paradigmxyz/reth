# RocksDB execution latency investigation

Measured on dev-brian on **2026-09-26**, with the same profiling binaries from
`32bac3f7d78d330d4f9757b241426551bb0b433e` as the
[original three-backend benchmark](state-trie-rocksdb-benchmarks.md).

Two causes explain most of the execution regression:

1. The complete trie inherits a **128 MiB shared RocksDB block cache**. Repeated
   SST reads, kernel-to-user copying, and Zstd decompression consume substantial
   CPU in execution, prewarming, and proof workers. Increasing the cache to
   4 GiB reduces execution p50 from **182.01 to 121.14 ms** on the same blocks.
2. The RocksDB implementation's **cross-database snapshot lock** blocks new
   providers while persistence writes RocksDB batches and commits MDBX. BAL
   execution opens providers inside its measured interval. Direct tracing
   catches **300–420 ms waits** at this lock during slow blocks. Deferring
   persistence removes these waits and reduces RocksDB execution p99 from
   **530.04 to 125.55 ms** in the controlled 250-block comparison.

There is a smaller residual read-path cost: with persistence deferred in both
backends, execution p50 is **111.60 ms for RocksDB versus 104.14 ms for complete
MDBX**. RocksDB still performs more lookup, copying, and decompression work.
These experiments do not assign every millisecond to independent additive
causes: prewarming, proofs, execution, and persistence overlap.

## Controlled comparisons

Each row replays the same first 250 BAL 1 Ggas blocks after recovering the same
promoted snapshot and clearing the Linux page cache. The table excludes the
first 50 blocks. Durations are milliseconds; each row has 200 samples.

| Complete-trie backend/configuration | Execution mean | p50 | p90 | p99 |
| --- | ---: | ---: | ---: | ---: |
| MDBX, normal persistence | 118.90 | 110.23 | 138.15 | 298.86 |
| RocksDB, 128 MiB cache | 195.84 | 182.01 | 218.33 | 602.43 |
| RocksDB, 4 GiB cache | 137.85 | 121.14 | 147.59 | 530.04 |
| RocksDB, 4 GiB cache, repeat | 133.88 | 120.89 | 144.82 | 535.73 |
| RocksDB, 16 GiB cache | 135.92 | 121.39 | 147.20 | 564.20 |
| RocksDB, 4 GiB, repeat with ignored worker flags | 133.14 | 120.54 | 142.01 | 510.77 |
| MDBX, persistence deferred until shutdown | 103.91 | 104.14 | 110.63 | 117.31 |
| RocksDB, 4 GiB, persistence deferred until shutdown | 111.43 | 111.60 | 117.79 | 125.55 |

The 4 GiB repeat reproduces the median and tail. Increasing to 16 GiB does not
improve execution; it also reduces available host memory to about 6.5 GiB.
The attempted 8-worker configuration was later found to be ignored: `reth-bb`
constructed its custom runtime before applying the CLI worker settings. That
row is another default-worker control, not evidence about fewer proof workers.
The [optimization follow-up](state-trie-rocksdb-optimization.md) fixes the
benchmark runner and verifies actual pool sizes before testing concurrency.

Deferring persistence is a diagnostic, not a proposed production setting. It
also retains a larger execution overlay and avoids background persistence
work. Therefore the entire normal/deferred difference must not be attributed
to the lock alone. Direct lock tracing independently establishes the stalls.

## Cache misses cause repeated reads and decompression

`RocksDBBuilder` in `crates/storage/provider/src/providers/rocksdb/provider.rs`
sets a 128 MiB cache shared by all column families, including index/filter and
data blocks. SST blocks are 16 KiB; bottom-level compression is Zstd. The
migration added about 182 GB of complete-trie SSTs without changing this cache.

For the full 250-block windows:

| Measurement | 128 MiB | 4 GiB |
| --- | ---: | ---: |
| Process logical read bytes (`rchar`, decimal GB) | 249.06 | 42.30 |
| Process read syscall count (millions) | 11.75 | 3.74 |
| Process physical read bytes (decimal GB) | 30.57 | 32.57 |
| Shared BAL-pool `pread64` calls | 1,110,970 | 509,122 |
| Shared BAL-pool `pread64` returned bytes (decimal GB) | 29.65 | 5.20 |
| Summed BAL-thread `pread64` duration (seconds) | 385.16 | 180.61 |

The sixfold drop in logical bytes with similar physical bytes distinguishes
repeated page-cache reads from device traffic. A Linux page-cache hit still
requires copying the compressed block into userspace and decoding it when the
RocksDB uncompressed block cache misses. Summed syscall durations overlap
across workers; they are not elapsed execution time. MDBX uses mmap, so its
read cost cannot be compared using `rchar` alone.

System-wide perf capture, filtered to the node PID during analysis, includes
the lazily spawned worker threads. At 128 MiB, the single native function
`ZSTD_decompressSequences_bmi2` accounts for **28.8% of storage-proof worker
self cycles** and **21.1% of BAL-prewarming worker self cycles**. The kernel
copy routine `_copy_to_iter` accounts for another **13.7% and 14.4%**,
respectively. These percentages refer to each thread group, not whole-node
time. Zstd functions and `_copy_to_iter` also appear on `bal-stream`
workers, which run both transaction execution and state-root preparation.

| Thread group, sampled cycles (trillions) | 128 MiB | 4 GiB |
| --- | ---: | ---: |
| Shared BAL pool | 1.202 | 0.884 |
| BAL prewarming | 1.215 | 0.396 |
| Storage proofs | 1.059 | 0.280 |
| Account proofs | 0.564 | 0.212 |
| Whole node | 6.243 | 3.782 |

This establishes read costs on the shared BAL pool and substantial extra
concurrent CPU work. It does not isolate reads by transaction execution from
state-root preparation on those same threads, or their wall-time contribution.
The [follow-up](state-trie-rocksdb-optimization.md) tests separating the pools.

**Correction to the initial diagnosis:** the earlier `perf record -p PID`
capture did not include the lazily created BAL, prewarming, or proof workers.
It could not support conclusions about their decompression costs. This
investigation uses `perf record -a`, verifies those thread names in the
capture, and filters the resulting samples to the node PID.

## Snapshot synchronization causes execution stalls

The relevant implementation is:

- `ProviderFactory::provider()` in
  `crates/storage/provider/src/providers/database/mod.rs` takes
  `state_trie_commit_lock().read()`, opens the MDBX transaction, and constructs
  the provider's owned RocksDB snapshot before releasing the guard.
- The normal commit path in
  `crates/storage/provider/src/providers/database/provider.rs` takes the write
  guard **before draining all pending RocksDB batches** and keeps it through
  the MDBX commit.
- Canonical BAL execution calls `make_db(false)` and BAL workers call
  `make_db(true)` inside execution. The BAL prewarming pool also opens one
  provider per worker per block.

The lock is specific to this implementation's cross-database consistency
mechanism. Existing RocksDB snapshots remain readable during writes; new
provider creation waits for the writer. Execution still uses the BundleState
overlay. Opening its backing provider is enough to encounter this lock,
before any particular account/storage lookup.

BPF probes on `parking_lot::RawRwLock::lock_shared_slow` identify a common
lock address across engine/BAL/prewarming waits. Capturing and resolving the
engine's return address identifies **`ProviderFactory::provider()`**, rather
than merely inferring the lock from adjacent log messages.

For example, block **24,979,248** in `rocks-large-prof` takes **561.95 ms** to
execute. Its engine thread waits **341.40 ms**, and a BAL worker waits
**344.87 ms**, on this lock. These waits overlap and must not be added.
Across the initial 4 GiB run, all eleven engine lock waits above 100 ms end
alongside a completed save. One release at `11:08:49.476065Z` precedes the
`Saved range of blocks` log at `11:08:49.476116Z` by about 51 microseconds.

Commit metrics from that run show a maximum RocksDB commit duration of
**385.0 ms**, versus **3.1 ms** for MDBX commit. With persistence deferred,
engine and BAL workers record no waits on this lock.

The repeated 4 GiB run traces 65 native RocksDB writes totaling **4.651 s**;
`fsync`/`fdatasync` within those writes totals **0.892 s**, or **19.2%**.
Thus synchronous WAL durability is one component, not the entire write cost.
CPU stacks include WAL record/checksum work and memtable insertion:
`WriteGroupToWAL`, `crc32c::ExtendImpl`, `WriteBatchInternal::InsertInto`,
`MemTable::Add`, and `InlineSkipList::Insert`.

Simply deleting the lock would remove the guarantee that the MDBX transaction
and RocksDB snapshot describe the same persisted frontier. An optimization
must preserve that pairing while avoiding a provider-creation barrier across
the full RocksDB write. Cache tuning alone cannot remove these stalls.

### 600-block confirmation and exact write breakdown

A final 4 GiB replay covers the original 600-block range. Excluding 50 warmup
blocks, execution is **129.64 / 170.46 / 578.53 ms** at p50/p90/p99, with a
**149.00 ms mean**. Payload validation is **142.91 / 187.56 / 592.08 ms**,
with a **163.47 ms mean**. This confirms the large improvement over the
original default-cache runs, whose pooled execution p50 was 199.15 ms, while
retaining the persistence-related tail. It is one instrumented confirmation,
not three new repetitions.

Block **24,979,552** takes **660.56 ms** to execute, with **416.28 ms** waiting
on the engine's provider lock and **419.25 ms** on a BAL worker's same lock.
Again, these are overlapping waits. The largest eight execution samples all
have engine lock waits above 300 ms.

Native function probes cover all 175 RocksDB writes during replay:

| Write component | Summed time (seconds) | Share of native write duration |
| --- | ---: | ---: |
| Native RocksDB write, total | 13.868 | 100% |
| `DBImpl::WriteGroupToWAL` | 7.599 | 54.8% |
| `WriteBatchInternal::InsertInto(WriteGroup, ...)` | 6.258 | 45.1% |
| `fsync`/`fdatasync`, already included above | 2.649 | 19.1% |
| Writing thread on CPU, across both phases | 11.847 | 85.4% |
| Writing thread descheduled, across both phases | 2.021 | 14.6% |

WAL and insertion account for 99.9% of native write duration. Scheduler probes
measure the writing thread's descheduled intervals while inside the native
write call; on-CPU time is the complement, including kernel CPU work. Sync
time overlaps WAL and both scheduling categories, so these rows must not all
be added. CPU stacks identify WAL checksums/record handling and memtable
skip-list insertion, consistent with these phase measurements. Making fsync
faster would leave most of the barrier's work in place.

## Remaining read-path cost

With persistence deferred, RocksDB execution is about **7.2% slower at p50**
than complete MDBX. Sampled BAL execution cycles are **0.841 versus 0.803
trillion**; prewarming costs **0.344 versus 0.199 trillion**. The RocksDB path
still performs SST index searches, key comparisons, block reads, and
decompression. The difference includes concurrent worker effects and the
backends' different physical I/O behavior; it is not a pure single-Get
microbenchmark.

`OverlayStateProvider::basic_account` and `storage` first consult the execution
overlay. Their database fallbacks use `RocksStateTrieCursor::get`, implemented
with snapshot `get_cf`. Cursor construction does not create an iterator;
neighbor/proof seeks construct it lazily. No state-trie-overlay computation
was reintroduced.

## Reproduction and scope

Artifacts are in `/home/ubuntu/state-trie-latency-20260926`: `run.py`,
`analyze.py`, `profile_summary.py`, BPF probes, exact command JSON, node logs,
metrics, profiles, per-block timings, and aggregate results. The first eight
runs use `waits-first-eight.bt` (write/caller tracing was added for the second
four); the final run uses `waits.bt`. Profiles may be gzip-compressed for space.

The host has 16 physical cores / 32 hardware threads and about 62 GiB RAM.
Normal persistence uses threshold 50, memory buffer target 5, and state masking
30. Deferred runs use threshold 100000, then persist fully at shutdown. All
runs use the identical corpus and recovered baseline, clear Linux page cache,
and have no concurrent builds or heavy analysis. Perf samples at 49 Hz with
4096-byte DWARF stacks. BPF measures reads and lock waits; later runs also
trace writes and sync calls. This instrumentation is diagnostic and changes
absolute timings somewhat.

The eight 250-block replays and one 600-block confirmation validate **2,600
payloads**. All roots match the corresponding original benchmark blocks;
there are no error logs, trie-overlay computations, root-task timeouts, or
successful serial fallbacks. Every run fully persists at shutdown to Finish
24,979,250 or 24,979,600, respectively. This is a controlled attribution study,
not a replacement for the original three repetitions of 600 blocks per backend.

Final recovery restores Finish and partial-state-trie to **24,979,000**, with
both complete-trie roots matching the legacy expected root
`0xc60084882e9a560a2679b26dbec4a5c7a6f5ac27a02985776706b82e45d7e254`.
The promoted superblock remains
`a9a302a983bc993722a14a514b16eb4b6ac2ac464ffa773d290ffd1998371c72`.
`/schelk` is mounted with **2,103,585,624,064 bytes** free. No promotion,
implementation change, or default-setting change was made during this study.
