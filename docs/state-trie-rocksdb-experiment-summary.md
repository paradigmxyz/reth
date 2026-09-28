# RocksDB investigation: experiment inventory and retained changes

Subsequent selection: restore the [overlapping RocksDB baseline](state-trie-rocksdb-selected-baseline.md)
without its proof shortcut. The Rayon environment override listed below was
reverted as part of that restoration. This inventory records the investigation
at its conclusion; non-overlapping runs are excluded from future comparisons.

This summarizes work after the original RocksDB implementation, commit
`32bac3f7d`, on September 25–28, 2026. The user requested stopping after the
512-versus-128 jemalloc-arena experiment. That final result is recorded below.

## Result

Execution parity with a matching, equally tuned MDBX control has **not been
established**. The major original regressions were an undersized RocksDB cache
and a provider-publication lock held across native writes. Both were addressed.
The remaining gap depends on concurrent work and scheduling; the measurements
do not establish one exact cause for every remaining millisecond.

| Comparison | Execution p50, RocksDB / MDBX | Payload p50, RocksDB / MDBX | Cumulative saves, RocksDB / MDBX |
| --- | ---: | ---: | ---: |
| Original implementation, three 600-block repetitions | 199.15 / 111.03 ms | 212.78 / 124.07 ms | 60.86 / 858.78 s per run |
| Retained code, matched eight-proof-worker settings | 81.44 / 77.88 ms | 155.08 / 145.87 ms | 47.75 / 562.60 s |
| Unretained proof-and-state-preparation deferral | 75.09 / 73.78 ms | 163.16 / 154.00 ms | 48.47 / 617.67 s |

These rows use different scheduling configurations and are not isolated estimates
of individual changes. Each comparison uses matching settings for both backends.
The retained-code RocksDB repeat measured 82.21 ms execution and 47.74 s saves.
The deferral experiment reduced execution but increased total payload latency;
its gates are not in retained source. The most recent hardware-counter pairs
were diagnostics, not replacements for uninstrumented acceptance runs.

**Final arena experiment:** 512 versus 128 jemalloc arenas measured execution
p50 **74.10 versus 75.10 ms**, with cumulative saves **47.73 versus 48.36 s**.
Execution p90/p99 did not improve. Both 600-block runs passed persistence,
restart and recovery checks. This used the unretained deferral binary; one
RocksDB-only pair does not establish MDBX parity or justify changing defaults.
No arena change was retained, and experimentation has stopped.

## Changes kept in source

Only five post-implementation commits change application code. Subsequent
experimental patches were restored after capturing their binaries.

| Commit | Retained behavior |
| --- | --- |
| `42c743f6e` | Share the committed RocksDB snapshot with readers, paired with the MDBX transaction under a short publication lock. Native writes and destruction of the old snapshot run outside that lock. |
| `42c743f6e` | Pinned point reads, reusable snapshot read options and lazy/reused iterators; exact Get before proof-neighbor seeks. |
| `42c743f6e` | Eight-key BAL storage prefetch through snapshot-pinned native MultiGet. Resolve the BundleState overlay once per batch, retain concurrent cache fills, and handle missing/duplicate keys correctly. |
| `42c743f6e` | An 8 GiB native cache for state-trie RocksDB builds; auxiliary-only builds retain 128 MiB. Trie tables use 4 KiB blocks, restart interval 4, hash-assisted data-block indexes, 10-bit Bloom filters, whole-key memtable filtering and LZ4 bottom-level compression. |
| `42c743f6e` | Rebalance the existing 4 GiB execution cache to 8% code, 82% storage and 10% accounts; no increase to its total budget. |
| `42c743f6e` | Offline `migrate_state_trie --rewrite`, with Finish/partial-state checks and root verification before/after; configurable subcompaction parallelism, using 16 for offline rewrites. |
| `42c743f6e` | Make `reth-bb` honor parsed runtime worker-count flags. Its existing 900-second graceful-shutdown allowance is preserved. |
| `0b9ca9456` | Recover signatures in exponentially growing ranges so early transactions reach ordered execution sooner. |
| `7142b43e4` | Return exact state-trie overlay hits without a redundant database lookup; preserve neighbor and tombstone behavior. |
| `e7bade20a` | A FIFO shutdown barrier drains queued persistence and post-save pruning before engine shutdown acknowledgement. Normal save acknowledgements still precede pruning. |
| `1e511d3c1` | Honor the standard `RAYON_NUM_THREADS` override; preserve Rayon's ordinary default when unset. |

Ordinary execution continues to use the BundleState overlay and records zero
state-trie-overlay computations. That separation predates the RocksDB backend;
it was preserved throughout. Synchronous WAL durability remains enabled.

The retained table layout increases trie SST bytes from 181.82 to 207.50 GB
(14.1%); the complete MDBX trie tables occupy 509.18 GB on the same snapshot.
The rewrite capability is retained, but experimental rewrites were recovered;
the promoted snapshot was not replaced during this investigation.

## Complete experiment inventory

Rows group repetitions and closely related parameter sweeps. Latencies are
execution p50 unless identified otherwise. Most later runs use 600 identical
BAL 1 Ggas blocks, exclude the first 50, and check persistence and restart block
601. Initial screening used 250 blocks. Results from different configurations
must not be ranked as if they shared one control.

### Original regression and read path

| Experiment | Finding and disposition |
| --- | --- |
| Original three-backend comparison, three repetitions each | RocksDB execution 199.15 ms versus complete MDBX 111.03 ms; saves 60.86 versus 858.78 s/run. Established the regression and persistence savings. |
| Initial 128 MiB versus 4 GiB cache profile | Execution 183.08 → 120.46 ms. Initial profiler missed lazily created workers; its thread-level attribution was superseded. |
| Corrected 250-block cache study: 128 MiB, 4 GiB plus repeat, 16 GiB | 182.01 → 121.14/120.89 ms; 16 GiB 121.39 ms. Logical reads dropped about sixfold while physical reads stayed similar. Larger cache kept, eventually sized at 8 GiB. |
| Attempted eight-worker and early sixteen-worker trials | CLI settings were ignored by `reth-bb`; these are default-worker repeats, not concurrency evidence. Runner bug fixed. |
| Defer persistence to shutdown, both backends | RocksDB 111.60 ms versus MDBX 104.14 ms; RocksDB p99 fell sharply. Diagnostic only: also changes overlay retention and concurrent work. |
| BPF lock tracing and 600-block confirmation | Directly observed 300–420 ms provider-lock stalls. WAL handling and memtable insertion accounted for almost all native-write duration; syncing was about 19%, not the whole cost. |
| Published snapshot and shortened publication barrier | Removed the long write-spanning provider barrier. Kept; later provider-creation medians were about 4 microseconds. |
| Pinned reads, snapshot ReadOptions reuse and iterator reuse | Removed allocation/copy work; standalone ReadOptions repeats did not establish a median gain. Kept as part of the simpler read path. |
| Eight-key BAL MultiGet | 118.00 ms in its original configuration; 5.72 million keys used MultiGet. Kept. |
| Exact Get before proof seek | 116.13 ms; iterator seeks fell 8.98 → 5.68 million. Kept. |
| Batch initial proof-v3 lookups | 105.19 ms, slightly slower proof jobs. Removed; BAL MultiGet remains. |
| Exact overlay-hit shortcut | Avoided redundant database reads; matched 95.36/87.78 ms RocksDB/MDBX, without closing the gap. Kept. |

### Native cache and storage layout

| Experiment | Finding and disposition |
| --- | --- |
| mmap reads with original layout | 127.07 versus 121.41 ms; rejected. |
| Custom decoded node cache | 123.12 ms; rejected. |
| Native 512 MiB row cache | Failed after 67 valid blocks: incompatible with existing pruning DeleteRange. Excluded and rejected. |
| 4 KiB/LZ4/Bloom/hash-assisted layout, 4 versus 8 GiB cache | 126.91 → 114.73 ms; physical reads decreased but smaller cache could not hold the larger metadata footprint. Layout and 8 GiB cache kept. |
| Whole-key memtable Bloom filtering | Validated with the retained layout. Kept; adjacent runs differed in length, so no isolated speedup is claimed. |
| HyperClock, first trial | 115.26 versus 114.73 ms; rejected. |
| 1 KiB blocks with partitioned indexes/filters | 124.06 ms; SSTs grew to 228.92 GB. Rejected. |
| 2 KiB blocks | 118.10 ms; SSTs grew to 214.66 GB. Rejected. |
| Uncompressed 4 KiB SSTs with mmap | 308.30 ms, saves 91.06 s, SSTs 283.62 GB. Rejected. |
| Keep index/filter blocks outside the shared LRU | Proof reads improved, execution 116.78 ms did not. Rejected. |
| Storage account-prefix filtering | Fewer file seeks but 117.49 ms and slower storage proofs. Rejected. |
| 16 GiB native cache, early and final comparisons | Little useful benefit; final 104.62 versus 104.88 ms at 8 GiB, with roughly 8 GiB more anonymous memory. Rejected as the default. |
| 32 GiB native cache | OOM after 371 valid blocks. Incomplete run excluded; rejected on this host. |
| Hash-search SST index, 2/33-byte prefixes | 95.73 ms, higher proof CPU, SSTs +1%. Rejected. |
| HyperClock with automatic entry sizing | Lower proof CPU, execution 95.22 ms essentially unchanged. Rejected. |
| Snapshot branch cache, 65,536 decoded branches | 96.11 ms, higher proof time/root wait. Removed. |
| Snapshot leaf cache, 262,144 entries including absence | 32.3% hit rate; 95.59 ms with higher prefetch CPU. Removed. |
| Disable cache insertion on neighbor iterators | 81.58 ms, no useful gain. Removed. |
| Finer hash indexes, 3/34-byte prefixes | 81.78 ms, payload 171.02 ms, SSTs +6.40 GB. Rejected. |
| Prefix-limited neighbor iterators with total-order fallback | 82.19 ms; payload and metadata costs remained high. Rejected. |
| Coarse address-only storage prefix with three-byte account prefix | 80.81 ms, SSTs +1.05 GB; no repeatable win/parity established. Not retained. |
| Disable native RocksDB statistics | About 2.5% less combined proof CPU, execution 81.32 ms within control variation. Statistics remain enabled. |
| Dedicated jemalloc allocator for the native block cache | 79.985 ms versus gated control 79.461 ms. Rejected. |
| Native LRU shards, 512 versus 64 | 80.234 versus 79.575 ms. Rejected. |
| Cache sizing with execution-gated proofs, 2/8/16 GiB | 2 GiB regressed to 177.17 ms and 90.04 s saves; 16 GiB was 81.09 ms versus 79.46 at 8 GiB. Neither retained. |
| Index/filter cache priority | Already enabled in source and runtime logs; no redundant benchmark performed. |

### Execution cache and prefetch

| Experiment | Finding and disposition |
| --- | --- |
| RPC block/receipt/BAL cache caps at 16 | Avoid retaining about 12.6 GB of irrelevant data. Kept in benchmark commands for both backends, not changed as application defaults. |
| Double execution-cache budget to 8 GiB, original weights | 118.28 ms despite fewer code misses. Rejected. |
| Rebalance the existing budget to 8/82/10 | Account misses 79,124 → 46,307; code misses 332,905 → 155,182; storage capacity unchanged. Kept. |
| Double rebalanced execution cache to 8 GiB | 106.32 ms despite fewer misses. Rejected. |
| Alternative 32/52/16 weights | 104.85 ms, essentially unchanged. Restored 8/82/10. |
| Account/code prefetch before all storage batches | 117.47 ms; storage misses increased. Rejected. |
| Increase prefetch batches from 8 to 32 keys | 117.68 ms; rejected. |
| Prefetch zero for already-cached absent accounts | Avoided 168,892 reads but 105.36 ms did not improve the median. Removed. |
| Retry execution cache once before backing reads | Avoided only 2.59 reads/block; 105.19 ms. Removed. |
| Choose shorter of two prefetch queues | 105.93 ms, no gain. Restored round robin. |
| Order account prefetch by earliest BAL change | Fewer account/storage misses, 95.09 ms without material latency improvement. Removed. |
| Group large prefetch sets by hashed prefix | Matched 79.16/76.85 ms; reduced worker CPU but increased coordinator work. Removed. |
| Reduce prefetch workers 128 → 64 | 117.16 versus 116.13 ms. Rejected. |
| Reduce prefetch workers 128 → 32 with concurrent proofs | Prefetch CPU fell 26%; execution 82.50 ms did not improve. Rejected. |
| Reduce prefetch workers 128 → 32 with deferred state/proofs | 75.42 versus 75.69 ms, worse p90. Rejected. |
| Raise prefetch-worker/coordinator priority | Tested in two scheduling configurations; no material execution improvement. Benchmark-only trials, not retained defaults. |
| Disable BAL batch I/O, early and optimized runs | Regressed execution to 131.03 and 114.51 ms. This flag also changes account-cache use, so it is not a pure prefetch isolation. Rejected. |
| Disable only BAL read-set prefetch, preserve cache use elsewhere | RocksDB/MDBX worsened to 85.29/99.80 ms. Rejected; beating a degraded MDBX control is not parity. |

### Scheduling, worker pools and result delivery

| Experiment | Finding and disposition |
| --- | --- |
| Engine/BAL priority, small-L3 engine isolation, then large-L3 isolation | RocksDB 116.38 → 114.80 → 106.68 ms. Matched large-L3 MDBX also improved to 99.43 ms. Useful benchmark placement, not a backend-parity result or production default. |
| Place proofs, prefetch and native background work on small-L3 CPUs | 105.58 ms; retained as a benchmark setting. |
| Return engine/BAL to ordinary priority | 107.13 ms, slower despite fewer cache misses. Rejected. |
| Strict splits: BAL on large L3, BAL plus recovery on large L3, BAL on small L3 | 106.74, 128.46 and 112.99 ms. Rejected. |
| Give background work only four physical cores | 106.40 ms; rejected. |
| Confine all background threads to small L3 | Lower engine CPU/greater IPC, but 109.84 ms execution. Rejected. |
| Isolate only persistence/root/sparse/overlay/drop workers | Baseline 103.68 ms with worse payload; retested after other changes at 107.23 ms and later 93.03 ms, still with root-wait tradeoffs. Benchmark experiments only. |
| Raise recovery/iterator priority, with and without prefetch priority | 102.01/102.75 ms; investigated scheduling sensitivity, no retained priority default. |
| Exponentially growing recovery ranges | Prioritized early transactions; three RocksDB repeats about 98.3–99.0 ms with higher recovery priority, but matched MDBX 88.72 ms. Recovery order kept, priorities remain runtime choices. |
| Uniform 512-transaction recovery batches | 106.16 ms; restored exponential ranges. |
| FIFO recovery claims with par_bridge | Recovery CPU fell 7.2%, execution 82.17 ms unchanged. Removed. |
| Recovery pool 16/24 versus 32 workers | 84.97/82.27 ms, no execution gain. Rejected. |
| BAL/shared pool 16/64 versus 32 workers | 108.86/113.46 ms; rejected. |
| Separate state-preparation pool from BAL workers | Regression test confirmed a scheduling dependency, but execution 105.41 ms did not improve. Removed. |
| Cap BAL execution at 24 workers, retain 32-thread pool | 75.31 versus 75.39 ms. Rejected. |
| Multiproof chunks 5 → 20 → 80 → 320 | Execution improved (96.96, 95.73, 93.37 ms), increasingly shifting work into root wait. Chunk 80 used in later benchmarks; no default changed. |
| Multiproof chunk 1,280 | Provisional 89.11 ms execution, 177.35 ms payload; restart pruning error excludes acceptance. Also exposed the shutdown bug below. |
| Move global Rayon work to small L3 and reduce 32 → 8 workers | Lower global/engine CPU and execution; more root wait. Runtime placement/count used in later benchmarks. Only honoring the environment override is kept in source. |
| Raise BAL/recovery workers to nice -10 | Improved execution in several placements, with root-wait tradeoffs. Matched MDBX improved too. No hard-coded priority change. |
| Proof pools 32/16, early trials | Minimal execution benefit or greater root wait. Actual counts verified after runner fix. Not retained defaults. |
| Proof pools 16/8 with tuned scheduling | RocksDB 84.47/81.77 ms; eight-worker matched retained-code control 81.44/77.88 ms plus RocksDB repeat 82.21. No parity. |
| Proof pools 4/2/1, matched backends | RocksDB/MDBX 80.38/77.19, 77.31/75.84, 76.44/75.84 ms. Payload grew to roughly 218/342/598 ms for RocksDB. Rejected as a solution. |
| Gate 64 proof workers until execution ends | 79.46/76.38 ms matched; saves preserved, root wait increased. Experimental gates removed from source. |
| Also defer BAL hashed-state preparation | 75.09/73.78 ms; RocksDB payload rose by 14.73 ms against preceding control. Not retained. |
| Bounded result channel, capacity 1,024 | 104.03 ms, insufficient improvement. Removed. |
| Return in-order results without buffering | 81.43 ms, within control variation. Removed. |
| Box worker results | Removed a 208-byte receive-path copy, but 82.44 ms did not improve. Removed. |
| Parked Tokio unbounded MPSC result channel | Lower CPU, more context switches; matched 81.79/78.96 ms. Removed. |
| Batch sixteen worker results | Lower CPU/switches, worse execution 84.21/80.86 ms. Removed. |
| Direct per-transaction result slots | 79.82/77.26 ms, no useful gain. Removed. |
| Notify only for next missing result | Engine CPU about 77.95 → 63.59 ms/block; switches about 1,944 → 68/block; execution 79.55 ms essentially unchanged. Not retained. |
| Engine/recovery on small-L3 side, others on large-L3 side | 91.57 ms, regression. Rejected. |
| Exclude engine core and sibling from sender/corpus feeder | 79.55/77.10 ms; no median gain. Kept only as a benchmark control. |

### Persistence, memory and native background work

| Experiment | Finding and disposition |
| --- | --- |
| Persistence threshold 100, buffer 5 | Saves 37.64 s, execution 103.70 ms; altered root/overlay behavior and did not reach parity. Defaults unchanged. |
| Threshold/buffer 100/40 and 150/100 | 103.61/102.84 ms, worse tails (p99 327/484 ms), greater memory. Defaults unchanged. |
| Disable sparse-trie pruning | Fewer proof nodes but worse root wait and only 2.10 GiB memory headroom. Rejected. |
| THP advice; precompaction; advised-allocation defragmentation | Coverage varied, execution 105.44/105.08/106.40 ms; no useful gain. Kernel settings restored. |
| THP plus disabled dirty/muzzy decay | Sustained about 16.7 GiB huge pages, execution 103.69 ms but worse p90/more memory. Rejected and settings restored. |
| Disable the engine SMT sibling's POLL idle state | 105.62 ms; no gain. Original setting restored. |
| Jemalloc arenas per physical CPU | 105.49 ms; rejected. |
| Increase native base level from 256 MiB to 1 GiB | Compaction output 13.45 → 6.95 GB and CPU 36.42 → 21.70 s, but execution 105.78 ms. Rejected. |
| Direct I/O for background SST writes | 96.46 ms and saves 52.96 s, worse than buffered control. Reverted. |
| Pace submissions by 750 ms | Execution 88.70 ms with lower sustained load; diagnostic only, not an unpaced win. Temporary filesystem reserve adjustment was restored. |
| Limit native jobs to two | Built/tested but cancelled before replay: observed concurrency was already at most one flush plus one compaction. No performance claim. |
| Limit background SST writes to 192 MiB/s | 76.17 versus uncapped 75.59 ms; no gain. Rejected. Sampling stopped before shutdown, so it did not establish zero deferred compaction debt. |
| Native background threads at nice 19 | 75.42 ms; commit time did not improve. Rejected. A post-drain verifier initially used the wrong legacy root, then was replaced with read-only stats; restart/recovery and audits passed. |
| Gate background SST writes during execution | 75.23 versus 75.40 ms, negligible gain; saves/backlog essentially unchanged. Diagnostic wrapper not retained. Observer lag was measured; this was not complete native-work quiescence. |
| Jemalloc 512 versus 128 arenas | Execution 74.10 versus 75.10 ms; saves 47.73 versus 48.36 s. Execution tails did not improve; paired block median difference −0.52 ms. Both full runs passed, but no matched MDBX arena trial or repetition. No default change retained. Final experiment; stopped. |

### Proof-calculator variants and diagnostic investigations

| Experiment or measurement | Finding and disposition |
| --- | --- |
| Skip predecessor when successor covers target, plus ancestor reuse | First version stalled after 297 valid blocks because it violated known-parent bounds. Excluded. Corrected version passed but did not improve latency; removed. |
| Ancestor metadata reuse alone | Fewer storage ancestor reads, 94.68 ms with target-specific diagnostics; no established gain. Removed. |
| Reuse previous Seek position for same-key predecessor query | 95.70 ms, negligible gain. Removed. |
| Use child-branch mask to prove absence early | Fewer seeks and lower proof CPU, 84.29 versus 84.47 ms. Added logic not retained. |
| Matched scheduler/provider traces, early and refreshed | Residual median gap mainly running CPU rather than provider creation or runnable delay. Later provider p50 about 0.0039/0.0033 ms and p99 0.0097/0.0056 ms. |
| Engine profiles and receive-path assembly | Empty-channel backoff dominated receive samples. Removed copies or wakeups saved CPU without establishing lower execution latency. |
| Worker queue/CPU/lifetime/provider-read timings | Similar summed EVM CPU; longer worker lifetimes, not a simple aggregate-read-time explanation. Instrumentation removed. |
| Ordered iterator/worker receive/finish timing | Localized both worker progress and work outside the iterator; result delivery alone did not explain the residual gap. |
| Native proof-worker and BAL/prefetch profiles | Extra RocksDB index/key-comparison/cache/decompression work. Early missed-thread and symbol-attribution analyses were explicitly superseded. |
| Save and compaction overlap analyses | Correlations with slower blocks, not additive causal attribution. Later pacing/priority/gating interventions did not produce a useful median gain. |
| Code-generation comparisons | Receive hotspot and hottest EVM transaction function had identical normalized instruction sequences; enclosing execution/commit helpers differed. No whole-program equivalence or causal code-layout claim. |
| Cargo feature-graph comparison | No differing revm/Alloy/hashbrown/foldhash/ruint features; only backend-related reth features differed. |
| Coarse execution phase timers | Residual median difference concentrated in ordered commit loop and, more modestly, BAL conversion. |
| Fine result-retrieval versus commit timers | First matched pair: paired commit gap +1.33 ms, retrieval +0.19 ms. Diagnostic, not a fixed decomposition across all runs. |
| Commit-scoped cycles/instructions | Paired instructions differed about 0.06%, CPI about +2.7% for RocksDB; matched execution 75.08/73.15 ms. |
| Expanded L1/cache/branch counters | Full coverage; small miss differences, but commit CPI gap did not reproduce. Commit medians 35.83/35.81 ms; paired retrieval gap +1.33 ms. Does not prove a single cache cause. |
| Shutdown native lifetime tracing | Found pruning accessing a destroyed RocksDB decompression-context cache. FIFO shutdown barrier fixed and verified ordering. Kept. |

## Validation, limitations and artifacts

Completed accepted comparisons check all reference roots, full persistence,
restart block 601, and no state-trie execution overlays or serial fallback.
Relevant tests, profiling builds, nightly formatting and workspace all-feature
Clippy were run for source experiments. Runtime-only variants reused captured,
verified binaries. Failed/OOM/stalled runs above are excluded from acceptance.
No builds or heavy analysis ran during measured replay, shutdown drain or restart.

CPU affinity, priority, pool sizes, RPC-cache caps and sender isolation are
benchmark settings rather than application defaults. Proof service times and
CPU totals overlap; they must not be summed into wall latency. Different proof
chunk sizes also change the work represented by one proof job. Lower CPU usage,
lower execution against a slower MDBX variant, and slower replay caused by
pacing are not execution-parity wins.

The retained fixes were validated in their applicable configurations. This work
does not extend the original happy-path PoC to reorgs, history, pipeline sync,
RPC, payload building or storage wipes.

Detailed evidence:

- [Original three-backend benchmark](state-trie-rocksdb-benchmarks.md).
- [Initial cache and provider-lock investigation](state-trie-rocksdb-latency.md).
- [Chronological optimization results](state-trie-rocksdb-optimization.md).
- `/home/ubuntu/state-trie-rocksdb-benches-20260925`: original runs and migration.
- `/home/ubuntu/state-trie-latency-20260926`: lock, syscall, scheduler and profile study.
- `/home/ubuntu/state-trie-optimization-20260926`: run commands, hashes, captured
  experimental source patches/binaries, per-block data, profiles and continuation journal.

The final recovery is complete. `/schelk` is mounted at block **24,979,000**;
Finish and partial-state checkpoints match, and both complete-trie roots verify
as `c60084882e9a560a2679b26dbec4a5c7a6f5ac27a02985776706b82e45d7e254`.
Available space after recovery was **2,103,585,624,064 bytes** (2.10 TB / 1.91 TiB).
No task replay, build, monitor or recovery remains running or queued. The promoted
snapshot is unchanged, host settings are restored, and S1 was left untouched.
