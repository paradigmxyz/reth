# RocksDB state-trie optimization

Measured on dev-brian on 2026-09-26–27, continuing the
[latency investigation](state-trie-rocksdb-latency.md).
Artifacts, captured binaries/source patches, and reproduction scripts are in
`/home/ubuntu/state-trie-optimization-20260926`. The full experiment notebook
is retained there as `experiment-notebook.md`.

## Outcome so far

The strict execution target remains open. The matched, uninstrumented
600-block control pair uses the retained implementation, eight global Rayon threads,
eight account/storage proof workers, proof chunks of 80, isolated background
work, and higher BAL/recovery-worker priority. RocksDB execution p50 is
**81.44 ms versus 77.88 ms for MDBX**, with cumulative saves of
**47.75 s versus 562.60 s (91.5% lower)**. Payload p50 is 155.08 versus
145.87 ms; state-root wait is 62.69 versus 56.72 ms. Both pass all 600 reference
roots and persisted restart at block 601. The RocksDB repeat measured
82.21 ms execution, 153.59 ms payload, 61.09 ms root wait, and 47.74 s saves,
and also passed all roots and restart. Reducing prewarming to 32 threads measured
82.50 ms execution with 47.67 s saves and was rejected. These pool settings improve execution while
substantially increasing state-root wait; they are not an overall payload win.

The preceding 64-proof-worker comparison was 87.28 versus 81.71 ms, saves
50.62 versus 551.29 s, and payload p50 131.18 versus 119.59 ms. Its MDBX median
is not the parity target for the current eight-proof-worker comparison.
The proof ancestor/successor and branch-mask experiments were removed because
they did not produce useful latency improvements.

Before the additional placement/pool/priority tuning, the matched pair was
95.87 versus 88.03 ms, saves 50.60 versus 552.41 s, and payload p50 117.10
versus 110.26 ms. The older 88.03 ms control is not a sufficient parity target
for the newly tuned RocksDB run.

A prior RocksDB trial with proof chunks of 320 reached execution p50 of
93.37 ms and saves of 49.58 s, but increased payload p50 to 123.82 ms.
The lower execution time shifted more waiting into state-root validation.
The earlier committed baseline measured 104.88 ms against 98.57 ms for MDBX;
that older control is not the current parity target.

## Retained changes

- Publish a shared RocksDB snapshot paired with the committed MDBX transaction.
  Readers can use the prior committed view while the next RocksDB batch writes.
  The publication lock covers only MDBX commit and snapshot replacement; old
  snapshot destruction happens outside it. Snapshot ownership avoids an Arc cycle.
- Use pinned point reads and reuse immutable snapshot read options. Try exact Get
  before allocating/seeking an iterator for proof targets.
- Batch BAL storage prefetch in groups of eight through native snapshot-pinned
  MultiGet. Resolve the BundleState execution overlay once per batch and preserve
  concurrent cache fills, input order, missing slots, and duplicate keys.
  Ordinary execution does not construct a state-trie overlay.
- Use an 8 GiB native cache when the state-trie RocksDB feature is enabled;
  auxiliary-table-only builds retain 128 MiB. Trie column families use 4 KiB data
  blocks, restart interval 4, hash-assisted data indexes, 10-bit Bloom filters,
  whole-key memtable filtering, and LZ4 bottom-level compression.
- Rebalance the unchanged 4 GiB execution-cache budget to 8% code, 82% storage,
  and 10% accounts. Rounded capacities become 32,768 code entries, 16,777,216
  storage entries, and 2,097,152 accounts. In the matched cache experiment,
  account misses fell from 79,124 to 46,307 and code misses from 332,905 to
  155,182, with storage misses essentially unchanged.
- Recover signatures in exponentially growing ranges to feed early transactions
  to the ordered BAL commit loop sooner. Both backends benefit from this change.
- Return exact state-trie overlay hits before querying the underlying database.
  Neighboring entries and tombstones still require database lookup. This removes
  redundant reads but did not materially improve the measured RocksDB workload.
- Honor existing runtime worker-count CLI settings in `reth-bb` and the standard
  `RAYON_NUM_THREADS` environment setting for the global pool.
- Drain queued persistence and pruning before acknowledging engine shutdown.
  Native tracing found pruning using a destroyed RocksDB compression-context
  cache during process exit; the FIFO shutdown barrier removes that race.
  Normal save acknowledgements still precede pruning.

`migrate_state_trie --rewrite` rewrites existing trie SSTs with the new layout
without rebuilding the trie or modifying MDBX. It checks the Finish/partial
frontier before writable open and verifies the root before and after rewriting.
Trie SST size grows from 181,819,230,880 to 207,495,533,561 bytes (14.1%). The
complete MDBX trie tables occupy 509,180,637,184 bytes on this snapshot.

## Method and current comparisons

Each run starts from the same promoted snapshot at block 24,979,000, clears
Linux page cache, and replays 600 identical BAL 1 Ggas blocks. The first 50 are
excluded from latency percentiles. All 600 roots must match the original
benchmark. Shutdown must persist through block 600; restart must validate and
persist block 601, then shut down cleanly. No compilation or heavy analysis runs
during measured replay, shutdown persistence, or restart checks. The working
volume is recovered afterward; the promoted snapshot remains unchanged.

Both backends cap RPC block/receipt/BAL caches at 16 entries. Default persistence
settings here are threshold 50, memory buffer 5, masking window 30. Cumulative
save time includes all 600 blocks, including shutdown flushing. Proof-job
percentiles describe individual overlapping jobs, not elapsed block latency.

This host has two CPU groups: CPUs 0–7/16–23 share 96 MiB L3; CPUs 8–15/24–31
share 32 MiB. The matched configuration pins engine to CPU 0, leaves sibling
CPU 16 unused by the node, and places proof, BAL-prefetch, and RocksDB background
workers on the 32 MiB group. Other workers can use the remaining CPUs. Engine
nice is -10 and the 32 BAL execution workers use -5. These are benchmark process
settings, not hard-coded application behavior. Affinity and priorities are
verified before the scored window; the latest runs also audit all threads later.

Latency columns are p50/p90/p99 in milliseconds; saves are cumulative seconds.

| Configuration | Execution | Payload validation | State-root wait | Saves | Ggas/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| MDBX, matched settings | 98.57/121.31/339.72 | 114.03/141.19/355.75 | 4.18/13.18/27.92 | 540.99 | 1.178 |
| RocksDB, 8 GiB | 104.88/126.89/219.70 | 120.31/150.41/240.44 | 3.36/15.85/36.75 | 49.08 | 5.217 |
| RocksDB, 16 GiB | 104.62/126.30/207.51 | 120.80/154.27/231.66 | 3.52/15.89/44.59 | 50.46 | 5.149 |
| RocksDB, threshold 100 / buffer 5, 16 GiB | 103.70/121.36/241.93 | 120.93/145.20/257.71 | 5.21/17.60/46.80 | 37.64 | 5.299 |
| RocksDB, threshold 100 / buffer 40, 8 GiB | 103.61/120.20/327.13 | 121.57/144.08/342.30 | 5.63/16.74/32.04 | 41.25 | 5.224 |

At 8 GiB native cache, anonymous RSS stays around 23 GiB versus 31 GiB at 16 GiB.
The 0.26 ms execution-median difference does not establish a useful benefit for
16 GiB. The smaller default is retained.

Fresh MDBX account proof-job mean/p50/p90/p99 is 0.803/0.481/1.381/7.318 ms;
storage is 0.299/0.111/0.704/2.902 ms. The 8 GiB RocksDB account mean/p50/p90/p99 is
1.104/0.723/2.055/7.761 ms and storage 0.447/0.231/1.013/3.395 ms. Full distributions are in each run's
`summary.json`; proof service-time sums overlap and must not be added to execution.
Individual save-call p50/p90/p99 is 1.315/1.624/2.027 s for RocksDB versus
26.554/56.231/79.166 s for MDBX. Batch sizes differ, so cumulative save time
over the same 600 persisted blocks is the primary persistence comparison.

## What the measurements establish

The old publication lock directly blocked engine/BAL workers for 300–420 ms
across RocksDB writes. After publishing snapshots, a traced 250-block run recorded
only one engine publication-lock wait above 1 ms (3.369 ms); the largest BAL wait
was 18.403 ms. This removes the observed long barrier without claiming provider
creation is entirely wait-free.

In initial 250-block comparisons, increasing the original layout's native cache
from 128 MiB to 4 GiB reduced execution p50 from roughly 182 to 121 ms. Logical
process reads fell from about 249 to 42 GB while physical reads stayed around
31–33 GB. With 4 KiB blocks, increasing cache from 4 to 8 GiB reduced logical reads
from 83.40 to 21.16 GB and execution p50 from 126.91 to 114.73 ms; physical reads
stayed around 23.5 GB. Whole-process counters include overlapping tasks.

A profile of all node threads attributes native RocksDB self cycles to the
shared BAL pool (2.09%), prefetch workers (25.30%), and storage-proof workers
(38.86%); engine itself had none. At the time of that profile, the BAL pool ran
both transaction execution and parallel state-root preparation, so its samples
cannot be attributed exclusively to EVM execution. Prefetch kernel work was 51.60%. These are CPU
work fractions, not additive wall-latency causes. BundleState overlay incremental
work was similar for MDBX and RocksDB (about 2.3 s summed), but faster persistence
caused more full overlay rebuilds. No state-trie execution-overlay computation
was observed.

A matched 250-block critical-path trace (first 50 excluded) measured the following
inside BAL execution. These diagnostic runs used identical uprobes and scheduler
tracing on both backends; they are separate from the uninstrumented 600-block
comparison. Component percentiles are not additive.

| Median, ms | MDBX | RocksDB |
| --- | ---: | ---: |
| Execution function wall time | 98.956 | 102.649 |
| Running CPU time | 70.966 | 74.710 |
| Blocked time until wakeup | 26.888 | 26.604 |
| Runnable scheduler delay | 0.940 | 1.109 |
| Provider creation during execution | 0.00446 | 0.00436 |

Mean wall time was 105.678/108.012 ms (MDBX/RocksDB), running time
72.482/74.264 ms, blocked time 32.122/32.271 ms, and scheduler delay
1.074/1.477 ms. Thus this window does not show extra median blocking or a
provider-creation barrier accounting for the gap. RocksDB provider creation had
rare outliers (maximum 8.686 ms), but p99 was 0.038 ms. In the original 600-block
runs, execution p50 for blocks 51–250, 251–450, and 451–600 was respectively
102.207/105.683/106.808 ms for RocksDB and 99.750/97.301/98.887 ms for MDBX.
The earlier-window trace alone does not establish the cause of later-block drift.

Engine-only CPU profiles over 600 blocks reproduced execution p50 of 105.78 ms
for RocksDB and 98.67 ms for MDBX. Excluding warm-up, estimated user CPU cycles
inside execution were 326.19 versus 308.56 million per block. The BAL worker
result receive path accounted for 59.49 versus 53.42 million cycles per block;
`AccountBal::update` was similar (43.23 versus 43.95 million), while memcpy was
lower for RocksDB (32.17 versus 37.37 million). These are sampled cycle estimates,
not additive explanations of latency percentiles.

Assembly-level annotation attributes 87.7% of the receive function's sampled
cycles to its backoff spin loop. Thus its extra CPU work principally indicates
waiting for worker results; it is not evidence that queue allocation itself is
the primary cause. A bounded 1,024-result queue passed all 27 BAL execution tests,
600 roots, and restart, but execution was 104.03/123.17/214.14 ms and cumulative
saves 49.47 s. The small median change does not establish a reliable improvement;
the queue change was discarded.

The profile uses 499 Hz user-cycle sampling. Default perf timestamps were mapped
to wall time using two isolated tracefs clock calibrations, including measured
clock drift. Samples are classified using logged execution intervals, trimming
1 ms from each boundary; 8 KiB stack captures do not always reach the outer
execution frame. Raw samples and the corrected analysis are retained. The first
analysis with an incorrect clock assumption is explicitly marked as superseded.

CPU placement materially affects execution. Moving the isolated engine from the
32 MiB L3 group to the 96 MiB group reduced RocksDB execution p50 from 114.80 to
106.68 ms. MDBX also benefits, which is why the unpinned reference is insufficient.
In the common final 400-block window, current engine-only counters report:

| Counter | MDBX | RocksDB 8 GiB |
| --- | ---: | ---: |
| Engine CPU ms/block | 94.70 | 96.59 |
| Instructions/block, millions | 550.60 | 524.53 |
| Cycles/block, millions | 477.08 | 478.69 |
| Instructions/cycle | 1.154 | 1.096 |
| Effective GHz | 5.038 | 4.956 |
| Engine CPU migrations | 0 | 0 |

These include engine work outside the execution timer. The counters support
remaining cache/scheduling interference, but do not uniquely attribute the
remaining execution gap. Current counters are engine-only (`--no-inherit`) with
grouped cycles/instructions at 100% hardware coverage. Earlier multiplexed or
inherited counters are excluded from this table.

Keeping threshold 100 and increasing the memory buffer to 40 measured execution
p50 103.61 ms, with 41.25 s of cumulative saves, but execution p99 rose to
327.13 ms. Engine CPU work remained about 96.91 ms/block; retaining more recent
state did not materially improve engine efficiency.

A stricter CPU split put BAL execution on the large-cache group and every other
worker on the small-cache group. Engine IPC improved to 1.222 and CPU work fell
to 91.15 ms/block, yet execution p50 regressed to 106.74 ms and root-wait p50
rose to 19.24 ms (payload p50 138.87 ms). Cumulative saves were 51.38 s.
Both 600-block and restart checks passed. The placement was discarded: improving
engine CPU efficiency alone did not improve elapsed execution or validation.

Restricting both BAL execution and transaction preparation to the seven available
large-cache cores was worse: execution p50/p90/p99 128.46/154.76/227.49 ms,
payload 138.85/168.40/243.52 ms, and 48.10 s cumulative saves. Root-wait p50
fell to 0.002 ms because execution took longer. All correctness checks passed;
this placement was discarded.

Placing BAL workers on the faster-clocked small-cache group also regressed:
execution p50/p90/p99 112.99/137.25/259.70 ms, payload
124.80/156.36/278.50 ms, and saves 50.52 s. Engine IPC was 1.083 and CPU work
100.85 ms/block. All correctness checks passed; the placement was discarded.
The original proof-side restriction remains the best of these placements.

Increasing retention further (threshold 150, memory buffer 100) measured execution
p50/p90/p99 102.84/120.65/484.02 ms, payload 122.38/150.39/511.87 ms,
root wait 8.09/22.82/50.37 ms, and cumulative saves 43.26 s. Anonymous memory
reached roughly 33 GiB during replay. All roots and restart checks passed.
The median gain comes with worse tails and more memory, so this is not the
preferred default; it still misses the matched MDBX median.

## Discarded experiments

No production code from these variants is retained. Raw results and patches
remain in the artifact directory.

- mmap, custom node cache, HyperClock, 1/2 KiB blocks, partitioned indexes, and
  an account-prefix filter did not improve execution. A native row cache failed
  because existing transaction-lookup pruning uses incompatible DeleteRange.
- A 32 GiB native cache exhausted memory; increasing the whole execution cache
  to 8 GiB did not help. RPC cache caps avoid retaining about 12.6 GB of large
  blocks, receipts, and BALs irrelevant to this Engine-only workload.
- Reducing prefetch/proof worker counts, changing prefetch order, batch size 32,
  and prefetching zero for cached absent accounts did not improve the target.
- Disabling sparse-trie pruning reduced returned account proof nodes by 34%, but
  execution p50 stayed 105.30 ms, root-wait p50 rose to 15.38 ms, and available
  memory fell to 2.10 GiB. Sparse cache-miss counters include retries and are not
  counts of distinct missing nodes.
- THP with 1.3–1.8 GiB coverage measured 105.44 ms; precompacting memory raised
  coverage to about 5.8 GiB but measured 105.08 ms. THP was restored to `never`.
- Batching initial proof lookups measured 105.19 ms and slightly slower proof
  jobs. Those calculator/cursor changes were removed; BAL MultiGet remains.

A follow-up enabled THP defragmentation for advised allocations and compacted
memory before startup. Huge-page coverage briefly reached 12.6 GiB, then fell
to about 4.5 GiB by the end. Execution p50/p90/p99 was
106.40/153.29/231.00 ms, payload 122.22/169.07/253.72 ms, and saves 56.37 s.
All correctness checks passed. Both THP allocation and defragmentation settings
were restored to `never`. This does not demonstrate sustained full-heap coverage.

Disabling allocator dirty/muzzy-page decay during the huge-page test sustained
about 16.7 GiB of huge pages through 600 blocks. Resident memory grew to roughly
32 GiB. Execution p50/p90/p99 was 103.69/143.12/213.20 ms, payload
118.87/161.27/248.18 ms, root wait 2.42/16.69/61.11 ms, and saves 51.36 s.
All correctness checks passed. The small median gain comes with worse p90 and
more memory, so neither huge-page nor allocator-retention tuning is retained.
Both kernel settings were restored to `never` before the next experiment.

Doubling the execution-cache budget to 8 GiB with the 8/82/10 proportions
increased capacities to 65,536 code entries, 33,554,432 storage entries, and
4,194,304 accounts. Code misses fell from 169,289 to 79,334; account misses
from 47,654 to 33,094; storage misses only from 358,792 to 340,409. Execution
p50/p90/p99 nevertheless regressed to 106.32/127.97/222.04 ms, payload to
121.38/155.78/253.33 ms, and saves totaled 50.64 s. All correctness checks
passed. The whole-cache increase was discarded.

A targeted 32/52/16 cache rebalance kept storage capacity unchanged, increased
code capacity to 131,072 and accounts to 4,194,304. Execution p50/p90/p99 was
104.85/124.62/212.80 ms, payload 120.23/152.62/243.41 ms, root wait
3.44/16.68/32.73 ms, and saves totaled 49.42 s. All 18 cache tests, 600 roots,
and restart checks passed. The median was unchanged, so the 8/82/10 proportions
were restored.

Separating BAL state-root preparation from the BAL execution pool reproduced and
removed a scheduling dependency in a regression test; all 40 targeted BAL/prewarm
tests passed. The 600-block benchmark nevertheless measured execution
105.41/128.58/212.48 ms, payload 121.30/150.55/235.00 ms, and cumulative saves
49.92 s. Roots and restart passed. State-preparation workers retained the shared
pool's priority. Engine CPU work rose to 98.35 ms/block from 96.59 ms/block.
The pool change and its regression test were discarded; their patch and test
results are retained in the artifacts. Demonstrating the dependency did not
establish it as the cause of the remaining workload latency.

Increasing the trie column-family base level limit from 256 MiB to 1 GiB cut
compaction output during replay from 13.45 to 6.95 GB and compaction CPU time
from 36.42 to 21.70 seconds. Execution nevertheless measured
105.78/129.94/207.66 ms, payload 120.84/154.67/230.64 ms, and saves 49.74 s.
All 600 roots and restart checks passed. The setting was discarded: reducing
this background compaction work did not improve execution.

Disabling BAL batch I/O with the optimized 8 GiB configuration regressed
execution to 114.51/163.30/258.35 ms, payload to 124.72/173.59/273.57 ms,
and saves totaled 49.10 s. All roots and restart passed. This existing flag
also bypasses the account cache in BAL state-root preparation, so the test
is not a pure isolation of storage prefetch. The enabled configuration remains.

Restricting proofs, prefetch, and RocksDB background work to four physical
cores while giving BAL execution eleven regressed execution to
106.40/127.35/220.80 ms and payload to 124.62/160.46/246.97 ms. Saves totaled
48.47 s; all roots and restart passed. The affinity change was discarded.
The two data drives reported zero thermal-throttling events and zero warning-
or critical-temperature time after this run.

Matched per-block differences in the baseline were broad: execution was slower
by a median 6.52 ms, with median differences of 6.49/5.97/7.07/7.19 ms across
transaction-count quartiles. These describe one paired replay, not confidence
intervals over repeated experiments.

Choosing the shorter of two prefetch queues passed 39 BAL/prewarm tests and
600 roots plus restart, but execution regressed to 105.93/127.46/220.66 ms.
Payload was 120.65/153.02/234.15 ms and saves totaled 49.69 s. The scheduling
change was discarded; round-robin prefetch dispatch remains.

Changing the shared BAL execution/state-preparation pool from 32 workers to
16 regressed execution to 108.86/129.01/229.61 ms, with saves of 49.50 s.
At 64 workers, execution was 113.46/146.91/219.11 ms and saves 50.37 s.
Both passed all roots and restart. The 64-worker run additionally sampled the
engine's SMT sibling for 20 seconds; it is diagnostic, not a replacement for
the primary comparison. The temporary worker-count hook was removed.

A subsequent unchanged-binary test disabled only CPU 16's POLL idle state.
Counters confirmed no POLL entries during the test. Execution nevertheless
measured 105.62/128.44/224.97 ms, payload 120.57/153.45/242.98 ms, and saves
50.09 s. All roots and restart passed. The original idle-state setting was
restored. Sampled idle-stack percentages are not evidence that polling caused
the residual latency gap.

A single execution-cache retry avoided 333 account, 841 storage, and 378 code
reads across 600 blocks (2.59 reads/block). This includes both brief bucket
contention and concurrent fills; the counter does not distinguish them.
All 18 cache tests, roots, and restart passed, but execution remained
105.19/127.26/215.02 ms, payload 120.82/152.22/235.57 ms, and saves 49.52 s.
The retry and diagnostic counters were discarded.

Configuring jemalloc arenas per physical CPU was verified at startup and
restart. Execution measured 105.49/125.61/226.11 ms, payload
120.62/152.25/249.94 ms, and saves 51.37 s. Roots and restart passed; the
allocator setting was discarded.

Confining all background node threads to the smaller-cache CPU group while
leaving BAL workers free to use both groups reduced engine CPU time to
91.60 ms/block and improved IPC to 1.228. Wall execution nevertheless worsened
to 109.84/143.57/237.28 ms, payload to 141.39/197.08/276.06 ms, and saves to
51.21 s. Engine context switches rose from 1.13 to 1.79 million over the
400-block measurement window. All roots and restart passed. This placement
was discarded: lower engine CPU work did not translate into faster validation.

## Worker timing comparison

An additional matched 600-block diagnostic pair times each BAL worker's queue
delay, thread CPU time, transaction execution, and lifetime. Provider instrumentation
sits beneath the execution cache, counting misses rather than hits. All roots,
persistence, and restart checks passed. The temporary instrumentation and its
dependency were removed after capturing both binaries.

| Median per block, ms | MDBX | RocksDB |
| --- | ---: | ---: |
| Execution | 98.55 | 104.65 |
| Last worker finish, relative to its scheduling time | 57.77 | 61.31 |
| Maximum worker queue delay | 19.18 | 17.18 |
| Sum of worker thread CPU time | 628.88 | 627.69 |
| Sum of worker transaction execution time | 1,021.26 | 1,014.55 |
| Sum of worker lifetime | 1,267.57 | 1,456.36 |
| Sum of uncached provider read time | 167.42 | 117.96 |

Worker CPU per transaction was 46.04/45.47 microseconds for MDBX/RocksDB.
Account and bytecode miss service times increased, but storage miss service
time decreased enough to reduce total read time. Cumulative saves were
567.37/49.26 seconds. These summed durations overlap across workers and do not
decompose wall execution latency. Worker scheduling timestamps also differ
slightly across the spawn loop. The extra worker lifetime primarily lies outside
the timed EVM calls; this motivates checking transaction supply and worker
scheduling rather than assuming slower EVM computation or aggregate database reads.

Raising the CPU recovery pool and transaction iterator from nice 0 to -5, matching
BAL workers, improved execution to 102.01/120.80/212.43 ms. Median summed worker
lifetime fell to 1,354.70 ms, while EVM time rose to 1,109.96 ms and uncached reads
to 173.70 ms. Payload p50 stayed at 120.60 ms as root wait rose to 6.05 ms; saves
totaled 51.35 s. Giving the prefetch workers and coordinator the same priority
reduced uncached reads to 124.03 ms, but execution was 102.75/121.36/219.09 ms,
payload p50 121.29 ms, root wait 6.89 ms, and saves 50.34 s. Both passed roots and
restart. Neither reaches the execution target; the priority changes remain
experimental rather than retained defaults.

Recovering transactions in exponentially growing parallel ranges prioritizes
the early results needed by ordered commit. With baseline priorities, this
candidate passed 31 conversion/cancellation/BAL tests and all replay/restart
checks. Execution was 101.63/129.12/201.18 ms, payload
118.92/155.52/237.18 ms, root wait 6.30/20.83/35.67 ms, and saves 49.39 s.
The median improves while p90 worsens modestly; it remains a candidate pending
further comparison.

Combining early-range recovery with the higher recovery-pool priority measured
98.35/119.37/210.72 ms execution, 119.63/150.98/235.68 ms payload, and
9.98/23.32/39.07 ms root wait. Saves totaled 50.83 s; all roots and restart
passed. This is just below the prior 98.57 ms MDBX median in one run. Repeats
and an MDBX control with the same recovery change and priority are required
before establishing parity. The second fresh RocksDB run measured
98.96/121.21/202.21 ms execution and 49.93 s cumulative saves.

The updated MDBX control, using the same early-range recovery and higher
recovery-pool priority, measured **88.72/106.59/331.83 ms** execution,
112.76/133.48/365.86 ms payload, 13.14/24.81/34.93 ms root wait, and
542.89 s cumulative saves. All roots and restart passed. The shared pipeline
change therefore benefits MDBX more, and RocksDB has **not** established parity
against this updated reference.

The third fresh RocksDB run measured 98.26/122.70/211.80 ms execution,
119.22/155.27/237.54 ms payload, 10.72/24.41/44.26 ms root wait, and
50.13 s of saves. All roots and restart passed. The mean of the three run
medians is 98.53 ms; this is not a pooled-block median or a confidence interval.
Average saves are 50.30 s, 90.7% below the updated MDBX control.

Confining only persistence, account/storage-root, sparse-trie, trie, overlay,
and drop workers to the smaller CPU group preserved the full CPU allocation
for transaction recovery and BAL workers. This isolated placement test used
the unchanged baseline binary. Execution was 103.68/124.86/211.48 ms, but
payload worsened to 125.05/160.98/248.47 ms and root wait to
10.93/26.63/50.94 ms. Saves totaled 51.45 s; engine CPU time stayed at
96.64 ms/block despite IPC improving to 1.134. All roots and restart passed.
The placement was discarded.

Increasing the existing multiproof chunk size from 5 to 20, keeping the
recovery candidate and CPU priority, measured execution
96.96/118.87/209.45 ms, payload 117.57/153.70/232.63 ms, root wait
9.52/24.37/46.73 ms, and saves 50.74 s. All roots and restart passed.
Account proof jobs fell to 285,770 and storage jobs to 696,108 over the scored
550 blocks. Job durations are not directly comparable across chunk sizes because
each job contains different work. This improves RocksDB but remains above the
updated MDBX execution median of 88.72 ms.

An 80-target chunk measured execution 95.73/118.23/213.40 ms, payload
117.31/151.87/239.90 ms, root wait 10.28/27.38/44.71 ms, and saves
51.01 s. All roots and restart passed. Scored proof-job counts fell further to
75,651 account and 563,013 storage jobs. This remains a runtime experiment;
no default chunk-size change has been committed.

A bounded cache of 65,536 decoded branches per immutable primary snapshot
passed seven state-trie tests, all replay roots, and restart. At chunk size 20,
execution was 96.11/116.29/210.38 ms and saves 49.60 s, but payload was
117.92/151.82/239.50 ms and root wait 11.57/26.83/45.98 ms.
Mean summed account proof service time rose from 2,413 to 2,528 ms per block,
and storage from 1,244 to 1,271 ms. These are overlapping worker times. The
small execution change did not offset higher proof time and root wait; the cache,
its dependency, and its additional tests were removed. Its binary, patch, and
results remain in the artifact directory.

Repeating the broader background-worker placement with early-range recovery,
higher recovery priority, and chunk size 80 still failed: execution
107.23/148.73/206.30 ms, payload 140.44/197.72/256.33 ms, root wait
22.11/46.42/72.71 ms, saves 51.91 s. Engine CPU time fell from 91.27 to
86.31 ms/block and IPC rose from 1.133 to 1.300, but elapsed execution worsened.
The placement trades lower engine CPU cost for slower worker progress. All
roots and restart passed; this placement is not retained.

Direct I/O for background SST writes (foreground reads remained buffered,
verified in the RocksDB startup log) measured execution
96.46/119.26/208.84 ms, payload 118.67/156.22/247.68 ms, root wait
11.54/27.39/54.34 ms, and saves 52.96 s. This is worse than the buffered
chunk-80 control. All roots and restart passed; the option was reverted.

Raising the recovery pool from nice -5 to -10 within the tight placement
improved execution to 100.48/133.36/221.37 ms, but payload remained
141.45/197.47/264.30 ms and root wait worsened to 27.76/55.09/86.99 ms.
Saves totaled 52.46 s; all roots and restart passed. It still loses to the
original placement and is discarded. Aggregate per-thread CPU counters over
all 600 blocks measured 695.65 ms/block for the CPU pool and 712.12 ms/block
for BAL workers; these overlapping thread times are not wall-clock durations.

Uniform 512-transaction recovery batches passed 31 conversion/cancellation/BAL
tests and all 600 roots plus restart, but execution worsened to
106.16/134.87/213.02 ms. Payload was 122.08/155.69/247.13 ms,
root wait 5.00/13.03/42.01 ms, and saves 51.48 s. Exponential batches were
restored; smaller root wait here did not compensate for slower execution.

Prefetching accounts by their earliest recorded BAL change reduced account cache
misses from 49,115 to 31,096 and storage misses from 313,852 to 208,150,
but execution remained 95.09/120.22/204.35 ms and its mean slightly increased.
Payload was 117.25/151.94/242.69 ms, root wait 11.23/26.31/44.66 ms,
and saves 50.58 s. All 600 roots and restart passed. The ordering change
was removed: the lower miss count did not translate into a material latency
improvement. BAL change indices also do not identify the first read of an account.

A matched diagnostic pair used proof chunks of 80 and temporary worker-receive,
worker-finish, and ordered-iterator timestamps. Both passed all 600 roots and
restart; the instrumentation was removed after capturing the binaries. Execution
p50 was 96.75 ms for RocksDB and 88.83 ms for MDBX, with cumulative saves of
49.96 s and 539.63 s. The mean execution difference was 6.36 ms: 2.84 ms inside
the ordered iterator and 3.52 ms outside it. Within the iterator, the difference
was +0.79 ms before the target worker received its transaction, +2.73 ms
overlapping target-worker processing, and -0.67 ms after that worker finished.
These phases include scheduling and reads; they do not isolate CPU execution.
They argue against result delivery or reordering as the primary remaining cost.
Mean differences add; component medians do not.

Over the engine-counter window, RocksDB used 91.70 ms CPU/block versus 86.88 ms,
with IPC 1.131 versus 1.194 and effective clock 4.944 versus 5.025 GHz.
Full-replay thread counters (a different window, including warmup) measured
account/storage proof CPU of 123/228 ms per block versus 65/88 ms for MDBX,
and RocksDB background CPU of 85 versus 5 ms. These overlapping thread times
support investigating interference from concurrent database/proof work; they
are not an additive decomposition of block latency.

Increasing proof chunks from 80 to 320 reduced execution p50 to 93.37 ms
(p90 115.42, p99 205.99) and cumulative saves to 49.58 s. However, root-wait
p50 increased to 18.93 ms and payload p50 to 123.82 ms, versus 10.28 and
117.31 ms for chunk 80. Account proof jobs fell from 75,651 to 25,791;
storage jobs only fell from 563,013 to 531,855. All roots and restart passed.
The execution gain alone does not justify that setting's larger payload latency.

The exact-overlay-hit fast path passed its focused test plus six proof, cursor,
snapshot, reopen, and secondary tests. Both backends then passed 600 roots and
restart with chunk size 80. RocksDB execution was 95.36/120.54/206.63 ms,
payload 117.26/152.80/242.61 ms, root wait 9.86/26.00/54.56 ms, and saves
51.51 s. MDBX execution was 87.78/104.64/324.86 ms, payload
109.87/131.58/357.37 ms, root wait 12.33/24.87/35.37 ms, and saves 542.59 s.
RocksDB proof CPU remained approximately 124/228 ms per block for account/storage
workers, so the exact-hit shortcut does not explain or close the remaining gap.

Raising both the 32 recovery workers and 32 BAL workers from nice -5 to -10
(with unchanged affinities) measured execution 94.05/114.21/207.77 ms,
payload 117.57/150.69/232.94 ms, root wait 12.29/31.06/50.21 ms, and saves
50.94 s. Both initial and later thread audits confirmed the priorities. The
small execution gain shifted time into root wait; payload mean was essentially
unchanged. All 600 roots and restart passed.

A separate load diagnostic set `bench send-blocks --wait-time 750ms`, keeping
ordinary worker priorities and the same binary, caches, and affinities. Actual
mean submission-completion interval was 776.47 ms, including sender overhead.
Execution improved to 88.70/99.94/121.44 ms, payload to 109.97/126.06/154.83 ms,
root wait was 10.27/24.02/38.97 ms, and cumulative saves fell to 39.85 s.
All 600 roots and restart passed. This is evidence of load sensitivity, not an
unpaced performance win: the sender deliberately reduced throughput.

In that diagnostic, measured engine frequency rose from 4.941 to 5.104 GHz
and IPC from 1.143 to 1.168. Storage cache misses fell from 316,440 to 268,510,
code misses from 193,008 to 146,296, while account misses rose from 43,074 to
47,203. Several factors change with pacing, including overlap with persistence
and background tasks, so frequency alone does not explain the latency change.
The longer metrics spool required temporarily reducing the root filesystem's
reserved allocation from 5% to 4%; the original 11,693,260 reserved blocks
were restored and verified after the spool was removed. The partition size,
node settings, and promoted snapshot were unchanged.

A subsequent unpaced proof-worker profile collected 23,524 user-cycle samples,
with 84 lost samples (about 0.36%). Weighted self-cycle shares for account/storage
workers were 8.45%/8.90% in SST index seek, 5.72%/7.40% in key comparison,
5.52%/5.69% in mutex unlock, and 4.48%/3.15% in LRU insertion. These are CPU
sample shares, not wall-latency contributions. All 600 roots and restart passed.
Per-worker attribution comes from individual `perf script` samples weighted by
period; symbol-aggregated, command-filtered reports misattributed shared symbols
and are explicitly marked unusable in the artifacts.

A hash-search SST index with account/storage prefixes of 2/33 bytes passed seven
cursor/proof tests, two migration tests, 600 roots, and restart. Ordered neighbor
queries explicitly crossed prefix boundaries. It did not improve performance:
execution was 95.73/120.68/213.62 ms, payload 117.82/154.67/239.82 ms,
root wait 9.81/27.22/52.93 ms, and saves 51.58 s. Account/storage proof CPU
increased from 124/228 to 126/235 ms per block. Rewritten trie SSTs were
209,538,768,966 bytes, 1.0% above the retained layout. The candidate was reverted;
its source patch, binaries, and measurements remain in the artifacts.

An independent 8 GiB HyperClock cache trial used automatic entry sizing while
keeping the retained SST layout. Seven targeted tests, 600 roots, and restart
passed. Execution was 95.22/122.08/210.13 ms, payload 116.50/152.02/235.58 ms,
root wait 9.25/25.56/43.61 ms, and saves 51.13 s. Account/storage proof CPU
fell from 124/228 to 117/213 ms per block, but execution p50 changed by only
0.14 ms. This does not establish a material execution benefit or parity.

Save-interval overlap alone also does not explain the gap: 265 of 550 scored
RocksDB executions overlapped `save_blocks`, compared with 549 for MDBX. MDBX's
long save intervals include substantial I/O waiting, so overlapping wall-time
intervals are not equivalent to concurrent CPU or memory pressure. Grouped and
matched-block comparisons are retained in `persistence-overlap-comparison.json`;
they are observational, with different block content, positions, and outliers.

A snapshot-scoped leaf cache (262,144 entries, including absent leaves) shared
point and batch reads while bypassing mutable secondary readers. Eight targeted
tests, 600 roots, and restart passed. It served 4,760,672 hits against 9,970,194
misses (32.3% hits), but execution was 95.59/118.75/219.95 ms and payload
117.34/153.37/245.51 ms. Root wait was 11.35/27.20/49.12 ms; saves were
51.01 s. Account/storage proof CPU decreased to 120/220 ms per block, while
BAL-prefetch CPU increased from 158 to 168 ms. The added cache did not improve
execution p50 and was removed along with its dependency. Captured artifacts
retain the diagnostic counters under `reth_reth_state_trie_leaf_cache_*`.

The next proof-calculator candidate skips a predecessor lookup when the successor's
physical path already covers the target. It also keys collected proof nodes by
their stored paths, allowing later targets to reuse ancestor metadata while
respecting each target's known-parent bound. This avoids redundant cursor reads
and decoding without adding another cache. Nine targeted cursor/proof/provider
tests and full workspace all-features nightly clippy and formatting passed.
Despite those checks, the RocksDB replay stalled on block 24,979,298 after 297
valid blocks, with a partial storage proof still pending at known-parent depth 1.
The queued MDBX comparison was stopped, and this run is excluded from performance
acceptance. Restoring original neighbor selection while retaining ancestor reuse then passed
600 roots and restart. The captured target was `a59c…0484`, with known parent
length 1. Its predecessor `a57c…3dee` had physical path `a5`; its successor
`a66c…c688` had physical path `a`. The successor covered the target's prefix but
could not contribute below the known parent, so choosing it produced an empty
partial proof. The predecessor supplied the required leaf.

The corrected shortcut requires the successor's physical path to be strictly
below the known-parent bound. A regression built from the captured nodes fails
with the old condition (empty proof versus the required leaf) and passes with
the fix; all ten targeted tests pass. Temporary target-specific logging has been
removed. Full workspace all-features nightly clippy and formatting pass. The
corrected RocksDB run passes all 600 reference roots, shutdown persistence, and
restart at block 601. Execution is 95.87/118.44/202.63 ms, payload
117.10/152.36/236.91 ms, root wait 9.66/25.56/50.49 ms, and saves 50.60 s.
It does not improve execution p50 over the previous 95.36 ms run. The matched
MDBX control measures execution 88.03/104.73/315.14 ms, payload
110.26/132.00/362.34 ms, root wait 11.30/23.97/38.51 ms, and saves 552.41 s.
The first control attempt failed from root-filesystem artifact exhaustion and is
excluded; the fresh repeat passes all roots and persisted restart. The harness
now checks artifact space before startup and during replay.

Engine CPU time over the counter window is 91.10 versus 84.60 ms/block, IPC is
1.133 versus 1.219, and effective clock is 4.930 versus 5.014 GHz for RocksDB/MDBX.
Across all 600 replay blocks, account/storage proof CPU is 123/223 versus
65/89 ms/block; native background work is 86 versus 5 ms/block. These windows
and overlapping thread times do not provide an additive latency decomposition.
Moving persistence, account/storage-root, sparse-trie, trie, overlay, and drop
workers to the smaller CPU group measured execution 93.03/113.94/209.80 ms,
payload 120.14/159.67/247.28 ms, root wait 15.93/35.57/61.93 ms, and saves
51.32 s. All roots and persisted restart pass. It reduces execution p50 by
2.84 ms but raises payload p50 by 3.04 ms; this is a scheduling tradeoff, not
a demonstrated reduction in total validation latency.

Ancestor reuse alone measured execution 94.68/118.80/211.17 ms, payload
117.21/152.66/232.22 ms, root wait 10.93/28.72/47.81 ms, and saves 51.34 s.
Logical account/storage ancestor reads fell from 4,826,933/8,174,475 to
4,820,683/7,676,851. Proof CPU remained approximately 125/228 ms per block.
That diagnostic run included the narrowly targeted proof logging described above;
it does not establish execution parity.

A RocksDB cursor trial reused the preceding native Seek position for a following
`before` query at the same key, avoiding another SeekForPrev. Ten targeted tests,
full workspace clippy/formatting, 600 roots, and persisted restart pass. Execution
is 95.70/117.73/206.04 ms, payload 117.14/152.92/239.33 ms, root wait
10.25/25.21/46.42 ms, and saves 51.37 s. The 0.18 ms execution-median change
does not establish a useful improvement; the cursor change was removed.

Increasing proof chunks from 80 to 1,280, with the same proof candidate and
original placement, measured execution 89.11/113.05/203.83 ms and saves 47.62 s.
However, payload validation rose to 177.35/226.80/334.17 ms and root wait to
75.71/112.93/171.93 ms. Although the 600 roots and restarted block 601 validated and the checkpoint
advanced, the final analyzer found a restart-time pruning error:
`ZSTD Data corruption detected`. The run is excluded from acceptance. Its
provisional timings also show a substantial validation regression. The harness
now preserves the restarted RocksDB LOG and rejects restart errors before
recovering the volume. The diagnostic replay and native shutdown investigation follow below.

The diagnostic repeat logged no error, but pruning stopped being logged at
AccountHistory start. The engine acknowledged shutdown after the durable-save
acknowledgement, while pruning continued on its OS thread. A native trace of the
old binary then observed persistence accessing RocksDB's process-wide decompression
cache 39 microseconds after its destructor returned, followed by ZSTD decompression.
This establishes a shutdown use-after-destruction bug; it provides a concrete
mechanism for the earlier corruption error without proving disk contents were damaged.

A FIFO persistence barrier now waits for post-save pruning before acknowledging
engine termination. Normal save acknowledgements remain before pruning. Both
shutdown tests fail with the old ordering and pass with the fix; nine applicable
RocksDB-feature tests and all ten default-feature tests pass. Full workspace
clippy and formatting pass. The fixed binary passes 600 roots and restart,
explicitly logs completed pruning for block 601, and the same native trace shows
pruning complete before cache destruction with no later cache access. The fix is
committed separately as `e7bade20a`; the post-destruction traces are retained in
`shutdown-trace-boundrocks-end` and `shutdown-trace-drainrocks`.

Including global Rayon workers in that placement measured execution
90.74/110.44/199.74 ms, payload 125.55/172.87/251.53 ms, root wait
24.98/50.06/77.12 ms, and cumulative saves 50.64 s. All 600 roots and
persisted restart pass. Engine CPU time fell to 80.85 ms/block and IPC rose
to 1.248, versus 91.10 ms and 1.133 in the original placement. This supports
CPU/cache interference as a remaining cause, but the extra root wait prevents
calling the placement an overall validation improvement. A smaller global Rayon
pool was then tested to reduce oversubscription of that CPU group.

Reducing the global Rayon pool from 32 to eight threads, with that placement,
measured execution 89.09/108.80/202.02 ms, payload 126.16/173.43/250.34 ms,
root wait 26.88/52.15/88.97 ms, and saves 49.65 s. Global-pool CPU work fell
from 400.07 to 287.62 ms/block; engine CPU fell to 79.95 ms/block, IPC 1.257.
The node explicitly configures the global pool, so a temporary experiment hook
was required; simply setting `RAYON_NUM_THREADS` would not change its size.
The actual eight-worker count was verified before the measured window.

Raising both BAL and recovery workers from nice -5 to -10 in that configuration
then measured execution 87.28/105.05/200.78 ms and saves 50.62 s. It beats the
earlier 88.03 ms MDBX reference in this run, but the matched MDBX control
reaches 81.71 ms, so parity is not established. Payload validation increased to 131.18/177.95/255.08 ms and root
wait to 33.20/59.68/92.37 ms. Both eight-worker RocksDB runs pass all 600 roots
and persisted restart. The scheduling tradeoff must be considered alongside
the execution median; this is not an overall payload-latency improvement.

With 16 account and 16 storage proof workers, RocksDB execution measured
84.47/101.26/202.73 ms, payload 128.66/186.36/281.22 ms, root wait
34.02/64.79/109.51 ms, and saves 49.52 s. It still exceeds the 81.71 ms
MDBX reference, which used 64 workers per proof pool. All roots and restart pass.

A further proof trial fetched the child branch directly below a known parent.
When its mask lacked the target child, that branch proved absence without
neighbor seeks; otherwise the normal ascent reused it. The regression fails
without the shortcut, eleven focused tests and full workspace checks pass, and
all 600 roots and restart pass. Storage seeks fall from 6,043,851 to 5,294,430,
while exact storage lookups rise from 7,705,921 to 8,164,659. Storage-proof CPU
falls from 184.97 to 178.43 ms/block, but execution p50 only changes from
84.47 to 84.29 ms; payload is 128.76 ms and saves 49.41 s. The shortcut was
removed because the latency benefit did not justify retaining the extra logic.

The global pool now honors the standard `RAYON_NUM_THREADS` setting instead of
explicitly overriding Rayon's thread count. Without an override the default
still uses available parallelism. Both backend builds and full workspace checks
pass; the shortcut experiment also validates the permanent setting at runtime.

Eight workers per proof pool measured execution 81.77/99.31/194.36 ms and
saves 48.81 s. The execution median is nearly equal to the 81.71 ms MDBX
reference, but that control used 64 proof workers per pool. Payload validation
increased to 153.34/226.04/341.69 ms and root wait to 62.73/115.42/178.85 ms.
All 600 roots and persisted restart pass. The lower execution median comes
with a substantial state-root latency tradeoff; it is not an overall payload
latency improvement.

The earlier successor-selection and ancestor-reuse proof experiments have also
been removed because they did not improve execution latency. Their source and
results remain in the artifacts. The retained original proof calculator passes
seven focused proof/cursor/provider tests. The matched eight-worker
RocksDB–MDBX–RocksDB sequence uses only retained code; seven focused tests, both
profiling builds, and full workspace checks pass. Execution p50 is
81.44/77.88/82.21 ms and saves are 47.75/562.60/47.74 seconds. All three runs
pass 600 roots and persisted restart. The permanent pool-setting change is
committed as `1e511d3c1`.

Reducing BAL prewarming from 128 to 32 workers passes 12 tests, full workspace
checks, 600 roots, and restart, but execution is 82.50/105.95/198.32 ms, payload
153.36/223.68/337.50 ms, root wait 60.84/104.79/163.03 ms, and saves 47.67 s.
Prewarming CPU falls from 156–157 to 116 ms/block (26%), without reducing engine
CPU or execution latency. The change was rejected.

Returning already-in-order worker results directly measured execution
81.43/97.58/193.25 ms, payload 153.84/222.37/344.88 ms, root wait
62.39/115.12/182.59 ms, and saves 47.85 s. All 27 BAL tests, workspace checks,
600 roots, and restart pass. It does not establish an improvement over the
81.44/82.21 ms references, so the change was removed.

Disabling block-cache insertion only for state-trie neighbor iterators measured
execution 81.58/98.63/195.79 ms, payload 156.10/228.48/337.21 ms, root wait
64.20/117.73/187.05 ms, and saves 49.25 s. Seven relevant tests, workspace checks,
600 roots, and restart pass. It was removed because it did not improve execution.
Index/filter cache priority was also considered, but both upstream source and
the reference RocksDB LOG show it is already enabled; no benchmark was run.

The retained RocksDB/MDBX binaries have identical normalized instruction sequences
for the result-channel receive function (1,526 bytes, 374 instructions) and nine
related receive helpers. This rules out different generated instruction sequences
in this hotspot, but does not rule out code-placement effects elsewhere. Assembly
and comparison data are retained in `codegen-recv-comparison.json` and the
`codegen-*.asm` artifacts. The receive function copies a 208-byte channel item.
Boxing worker results makes channel messages and reorder slots pointer-sized.
Assembly confirms that the receive function drops its 208-byte memcpy call and
shrinks from 1,526 to 1,427 bytes. However, execution measures
82.44/101.94/208.39 ms, payload 154.48/225.72/337.19 ms, root wait
61.11/112.05/181.63 ms, and saves 48.18 s. All 27 BAL tests, full workspace
checks, 600 roots, and restart pass. The allocation/copy tradeoff did not improve
latency, so the change was removed.

A per-block comparison against the mean of the two retained-code RocksDB controls
also puts the prewarming, direct-return, and cache-insertion trials within the
variation between those controls (`paired-execution-vs-lean-controls.json`).
Raising all 128 prewarming workers and their dispatcher from nice 0 to -5
measured execution 81.80/98.69/193.69 ms, payload 155.23/224.87/349.32 ms,
root wait 63.54/114.63/184.48 ms, and saves 48.72 s. Initial and later
thread audits confirmed the priorities. All roots and restart pass; the
configuration was rejected because execution remained within control variation.

A second native hash-index experiment used three-byte account prefixes and
34-byte storage prefixes (account hash plus two slot-key bytes), with total-order
neighbor iteration. The earlier rejected hash-index experiment used two and
33 bytes. Seven cursor/proof/snapshot tests, two migration tests, full workspace
checks, 600 roots, and restart pass. Execution measured 81.78/100.03/205.63 ms,
payload 171.02/258.54/368.04 ms, root wait 77.23/141.51/235.73 ms, and saves
48.81 s. Account/storage proof CPU rose to 95.57/168.43 ms per block from
93.18/162.52 in the first retained-code control. SST size increased by
6,400,212,549 bytes (3.1%) over the retained layout. The candidate was removed:
it increased storage and payload latency without improving execution.

A CPU-placement trial put the engine on CPU 8, signature recovery on
9–15/25–31, and all other node workers on the 96 MiB-cache cluster
0–7/16–23. CPU 24 was unused by the node; worker priorities were unchanged.
Both affinity audits and all roots/restart checks passed. Execution worsened to
91.57/115.65/198.50 ms, payload to 161.21/227.50/316.17 ms, root wait was
55.73/102.13/160.69 ms, and saves totaled 47.22 s. Engine CPU rose to
91.60 ms/block and IPC fell to 1.115; the placement was discarded.

Disabling the node's unconditional native statistics collection was verified
in the runtime LOG (statistics pointer null). Node metrics and durability
settings were unchanged. Full workspace checks, 600 roots, and restart pass.
Execution measured 81.32/99.60/193.07 ms, payload 153.64/224.56/337.87 ms,
root wait 61.60/113.13/183.30 ms, and saves 48.13 s. Account/storage proof CPU
fell from 93.18/162.52 to 91.08/158.32 ms/block, approximately 2.5% combined,
but the execution change stayed within control variation. Statistics remain
enabled in the retained implementation.

Native source inspection showed that total-order iteration disables hash-index
seeks. A follow-up used prefix-limited iterators for neighbor queries, with a
second total-order iterator as fallback across prefix boundaries. Expanded SST
cursor/proof tests, workspace checks, 600 roots, and restart all pass.
Account proof CPU fell to 86.40 ms/block, but storage proof CPU rose to
170.35 ms/block. Execution was 82.19/99.65/191.87 ms, payload
169.55/251.45/338.92 ms, root wait 74.29/133.54/230.95 ms, and saves 49.41 s.
The fine-grained hash metadata retains the 6.4 GB SST increase, and the preceding
hash-index run's peak anonymous memory was 29.92 GiB versus 23.45 GiB for the
retained layout. This two-iterator version was discarded. Native LOG counters
only contain a startup dump in these short runs and cannot establish runtime
prefix-filter usage.

A coarser storage-prefix trial retained three-byte account prefixes but used
only the 32-byte address for storage. Storage cursors then needed one bounded
iterator, while account cursors retained a total-order fallback. Seven targeted
tests, both migration tests, workspace checks, 600 roots, and restart passed.
Execution was 80.81/99.51/196.77 ms, payload 153.91/227.46/331.85 ms,
root wait 62.16/110.24/184.43 ms, and saves 48.44 s. Account/storage proof CPU
was 85.25/166.22 ms per block. SST size was 208,544,208,478 bytes, an increase
of 1,048,674,917 bytes (0.51%) over the retained layout, and peak anonymous
memory was 24.00 GiB. The small single-run execution change does not establish
parity or a repeatable improvement; this candidate remains unretained.

## Refreshed matched execution trace

A new 600-block pair used the retained binaries, eight proof workers, the same
120-second idle cooldown before each node start, scheduler/provider probes, and
499 Hz engine user-cycle samples. Both runs validated all roots, persisted
restart, and final recovery. Perf used CLOCK_MONOTONIC with per-run start/end
clock anchors; every block has an execution trace. These instrumented runs are
diagnostics, not new uninstrumented acceptance results.

| Execution component, median ms | RocksDB | MDBX |
| --- | ---: | ---: |
| Wall time | 82.128 | 77.881 |
| Running CPU time | 59.122 | 56.012 |
| Blocked time | 21.166 | 20.593 |
| Runnable scheduler delay | 0.761 | 0.635 |
| Provider creation | 0.0039 | 0.0033 |

Provider-creation p99 was 0.0097/0.0056 ms (RocksDB/MDBX); RocksDB had one
9.56 ms outlier. Provider creation does not account for the execution median
gap. Comparing the same blocks, the median differences were +3.322 ms running,
-0.712 ms blocked, and +0.097 ms runnable. Component medians are not additive.
Mean differences do add: +2.380 ms running, -0.731 ms blocked, and -0.056 ms
runnable account for +1.594 ms mean wall time; MDBX has much larger tail stalls.

Estimated receive-function self cycles were 35.49/31.31 million per block;
memcpy self cycles were 32.51/29.65 million. Copies directly called by the
receive function were essentially equal at 5.02/5.01 million cycles per block.
Other copy samples include state commit and iterator work, with possible
inlining differences between callers. These sampled cycle estimates are not an
additive explanation of latency percentiles.

Engine counter-window clocks were 4.931/4.999 GHz. Replay CPU-temperature
medians were 84.4/65.8 C, despite the identical initial cooldown. The scored
550-block replay took 129.38/426.71 seconds; cumulative saves were
49.25/556.78 seconds. The uninstrumented controls likewise took 128.27/429.95
seconds. MDBX's long pauses make these different sustained loads. This does not
establish thermal throttling or justify claiming parity by pacing RocksDB.

Fresh assembly annotation confirms the empty-channel backoff spin remains the
receive hotspot: about 85% of that function's sampled cycles for both backends.
This annotation covers the full replay's receive samples, including warm-up;
it is separate from the execution-window cycle totals above.

A FIFO recovery-claim trial replaced exponential Rayon ranges with par_bridge.
Claims followed input order, but completions remained out of order and were not
bounded to the number of workers. Recovery CPU fell to 623.90 ms/block from
672.52 ms/block (7.2%), while execution was 82.17/99.23/200.07 ms,
payload 155.69/226.58/337.48 ms, root wait 62.83/113.05/183.04 ms,
and cumulative saves 48.54 seconds. All 31 targeted tests, workspace checks,
600 roots, and restart passed. Lower recovery CPU did not improve execution
latency; the candidate was removed.

Recovery-pool sizing changed only the CPU pool, leaving BAL at 32 workers,
prewarming at 128, and account/storage proofs at eight workers each. Both trials
passed 600 roots, persisted restart, and initial/later thread audits. Runtime
tests, the profiling build, and full workspace checks passed.

| Recovery workers | Execution p50/p90/p99, ms | Payload p50, ms | Root wait p50, ms | Saves, s |
| --- | --- | ---: | ---: | ---: |
| 16 | 84.97 / 105.59 / 194.45 | 149.50 | 57.68 | 47.94 |
| 24 | 82.27 / 101.21 / 191.84 | 153.09 | 60.49 | 47.42 |

Neither improved execution over the retained 32-worker controls; the temporary
pool-size override was removed. The lower root wait at 16 workers improved
payload latency while execution regressed, illustrating the competing work
rather than an execution win.

A result-delivery trial replaced only the BAL worker-result channel with Tokio's
unbounded MPSC channel and blocking receive. Transaction dispatch, error ordering,
and cancellation stayed unchanged. All 31 targeted tests, both backend builds,
workspace checks, 600 roots per backend, and both persisted restarts passed.

| Parked result channel | RocksDB | MDBX |
| --- | ---: | ---: |
| Execution p50/p90/p99, ms | 81.79 / 97.55 / 192.86 | 78.96 / 97.62 / 353.16 |
| Payload p50/p90/p99, ms | 154.52 / 227.45 / 346.19 | 146.57 / 205.15 / 484.95 |
| Root wait p50/p90/p99, ms | 63.24 / 113.52 / 184.41 | 55.42 / 96.55 / 190.99 |
| Cumulative saves, s | 48.56 | 515.88 |
| Engine CPU, ms/block | 72.36 | 74.27 |
| Engine context switches/block | 3,466 | 3,030 |

In the retained-code first control, engine CPU was 77.76/75.80 ms per block
and context switches were 1,858/1,564 for RocksDB/MDBX. These counters use blocks
201–600 and include engine work outside execution; latency percentiles use
blocks 51–600. Lower engine CPU did not improve RocksDB execution p50, and
MDBX's measured median was also higher. The smaller backend gap therefore
does not establish an improvement. The channel change was rejected.

Halving both proof pools to four workers used the retained binaries, unchanged
cache sizes, and the same recovery/BAL counts, CPU placement, priorities, and
persistence settings. Both runs passed 600 roots, persisted restart, and worker
audits.

| Four proof workers per pool | RocksDB | MDBX |
| --- | ---: | ---: |
| Execution p50/p90/p99, ms | 80.38 / 96.96 / 190.00 | 77.19 / 96.06 / 325.26 |
| Payload p50/p90/p99, ms | 218.07 / 314.72 / 445.54 | 214.07 / 327.37 / 612.29 |
| Root wait p50/p90/p99, ms | 125.54 / 201.48 / 321.19 | 121.02 / 213.61 / 447.72 |
| Cumulative saves, s | 45.65 | 568.87 |

Saves remain 92.0% lower, but the execution median gap is 3.19 ms and payload
latency is substantially worse. RocksDB account/storage-proof CPU falls to
80.13/139.70 ms per block, from 93.18/162.52 in the first eight-worker control.
Engine CPU remains 76.59 ms per block. Four workers therefore do not establish
execution parity or an overall validation improvement.

The same comparison with two workers per proof pool passed 600 roots, persisted
restart, recovery, and thread audits for both backends.

| Two proof workers per pool | RocksDB | MDBX |
| --- | ---: | ---: |
| Execution p50/p90/p99, ms | 77.31 / 94.97 / 183.38 | 75.84 / 94.05 / 336.25 |
| Payload p50/p90/p99, ms | 342.42 / 500.77 / 757.31 | 348.50 / 545.95 / 1030.33 |
| Root wait p50/p90/p99, ms | 252.10 / 397.14 / 646.75 | 254.09 / 432.02 / 883.44 |
| Cumulative saves, s | 43.18 | 517.01 |

The execution median gap narrows to 1.46 ms and saves remain 91.6% lower.
However, this still misses the matched execution target and more than doubles
RocksDB payload p50 relative to eight workers. RocksDB engine CPU falls to
73.62 ms/block, account/storage-proof CPU to 70.02/126.78 ms/block.
The thread snapshots show no workers appearing after the late affinity audit
in the retained eight-worker control or the two-worker RocksDB run.

## Validation

The subsequent recovery-order change passed 31 conversion, cancellation, and BAL
tests. The overlay-read change passed seven targeted tests; full workspace
all-features nightly clippy and formatting checks passed again afterward.

301 selected provider, overlay, and execution-cache tests passed, with eight
known exclusions for features outside the happy-path PoC. Snapshot tests cover
readers during the RocksDB-write/MDBX-commit interval, old-reader stability,
publisher ownership, MultiGet ordering/duplicates/missing values, and secondary
catch-up. All retained final configurations validate 600 roots plus restart.
Workspace all-features nightly clippy passed, with future-compatibility notices
in existing dependencies. Default-feature provider/overlay/engine compilation also
passes. Formatting, dependency hygiene, and TOML checks pass.

### One proof worker per pool (2026-09-27)

Both matched runs passed 600 payloads, persisted-root checks, restart payload 601,
and recovery. The first 50 payloads are excluded from latency percentiles.

| Metric | RocksDB | MDBX |
| --- | ---: | ---: |
| Execution p50 / p90 / p99 (ms) | 76.438 / 92.019 / 175.630 | 75.842 / 97.761 / 186.354 |
| Payload p50 / p90 / p99 (ms) | 597.678 / 844.232 / 1413.106 | 595.942 / 991.144 / 1879.664 |
| Root wait p50 (ms) | 504.905 | 502.192 |
| Cumulative save_blocks, including drain (s) | 41.067 | 254.004 |

The execution median gap is still 0.597 ms. Persistence savings are 83.8% in
this matched configuration; MDBX cumulative saves also decreased with the much
slower replay. This is not execution parity, and the payload-latency penalty
precludes treating this as an overall improvement. No default changed.
Artifacts use the retained globallean backend binaries and the usual run-name
suffix with proof1-chunk80-600.

### Proof workers released after execution (2026-09-27)

A diagnostic candidate holds both 64-worker proof pools before provider creation
until actual BAL execution returns. Closing the gate on early job drop also
releases every worker; 55 targeted proof/state-root/BAL tests and both backend
builds passed, as did workspace Clippy and formatting. The BAL update-stream
completion signal is deliberately not used because it can precede execution.

| Metric | RocksDB | MDBX |
| --- | ---: | ---: |
| Execution p50 / p90 / p99 (ms) | 79.461 / 98.314 / 200.370 | 76.380 / 96.299 / 338.381 |
| Payload p50 / p90 / p99 (ms) | 149.004 / 199.407 / 283.265 | 132.763 / 171.303 / 407.788 |
| Root wait p50 (ms) | 57.867 | 45.918 |
| Cumulative save_blocks, including drain (s) | 48.752 | 562.721 |

Both runs passed 600 roots, persistence, restart payload 601, recovery, and
worker-count audits. The 3.081 ms execution median gap remains; saving time is
91.3% lower. This improves execution over the retained eight-worker RocksDB
control while recovering most of the one-worker payload penalty, but does not
establish parity. Candidate binaries, source patches, hashes, and full results
are retained under globalphase backend names with early0-proof64-chunk80-600.
The source was restored after building; no default changed.

### Dedicated block-cache allocator (2026-09-27)

A link-time diagnostic wrapper selected RocksDB's JemallocNodumpAllocator for
the 8 GiB cache while retaining the gated 64-worker configuration. The native
create/destroy probe, startup capacity assertion, 600 roots, persistence,
restart block 601, and recovery all passed. Execution p50/p90/p99 were
79.985/98.013/187.643 ms, payload p50 was 148.631 ms, root wait p50 was 57.786 ms,
and cumulative saves were 50.167 s. This did not improve execution over the
79.461 ms gated control and was rejected. The C wrapper, build/probe logs,
binary hash, and run artifacts are retained under globalarenarocks. No allocator
or default source change was retained.

### Smaller cache with execution-gated proofs (2026-09-27)

Reducing the native cache from 8 GiB to 2 GiB, with the ordinary allocator and
all other gated-worker settings unchanged, regressed execution p50/p90/p99 to
177.167/254.971/335.899 ms. Payload p50 was 336.940 ms, root wait p50 was
149.253 ms, and cumulative saves were 90.041 s. All 600 payloads, restart 601,
persistence and recovery checks passed, but this setting was rejected.
Artifacts use globalphaserocks-cache2g with the early0-proof64 suffix.

### Larger cache with execution-gated proofs (2026-09-27)

The matched RocksDB 16 GiB cache trial passed all 600 roots, persistence,
restart 601 and recovery. Execution p50/p90/p99 were 81.093/98.234/195.643 ms,
payload p50 was 150.613 ms, root wait p50 was 58.440 ms, and cumulative saves
were 49.784 s. It did not improve on the 8 GiB gated control (79.461 ms execution,
48.752 s saves), so 8 GiB remains the comparison setting.

For the 2 GiB trial, process I/O counters over all 600 payloads (before shutdown)
showed 1.677 TB of read-call bytes versus 55.071 GB with 8 GiB, while physical
read bytes were 43.540 GB versus 46.005 GB. BAL worker CPU increased from
700.6 to 1806.4 ms/block and prewarm CPU from 157.1 to 1646.8 ms/block. This is
consistent with repeated reads and processing of data already in the OS cache;
it is not evidence of a comparably large increase in physical disk I/O.

### Batching sixteen BAL results (2026-09-27)

A diagnostic candidate batched worker results in groups of sixteen, flushing
errors immediately and partial batches when workers finished. All 56 targeted
tests, both profiling builds, workspace Clippy, formatting, both 600-payload
runs, persistence, restart 601 and recovery passed.

| Metric | RocksDB | MDBX |
| --- | ---: | ---: |
| Execution p50 / p90 / p99 (ms) | 84.206 / 102.135 / 190.823 | 80.861 / 102.530 / 345.865 |
| Payload p50 (ms) | 153.116 | 137.939 |
| Root wait p50 (ms) | 57.495 | 46.484 |
| Cumulative saves including drain (s) | 48.425 | 551.705 |

RocksDB engine CPU fell from 77.673 to 67.853 ms/block in the 400-block counter
window, and context switches fell from 1841.445 to 448.190 per block. Execution
p50 nevertheless increased by 4.745 ms; MDBX p50 also increased by 4.482 ms.
These counter averages and latency percentiles use different windows and cannot
be subtracted to derive an exact wait-time increase. Holding ready results until
a batch fills is a plausible source of the added latency. This candidate was
rejected; original sources were restored and artifacts retained as globalbatchout
backend runs with early0-proof64-chunk80-600.

### Direct per-transaction result slots (2026-09-28)

A diagnostic candidate published each BAL result directly into its indexed slot,
with a capacity-one notification channel, instead of receiving and buffering
out-of-order results on the engine thread. All 59 targeted tests, both profiling
builds, workspace Clippy, formatting, both 600-payload runs, persistence,
restart 601, recovery, and worker-count audits passed.

| Metric | RocksDB | MDBX |
| --- | ---: | ---: |
| Execution p50 / p90 / p99 (ms) | 79.816 / 97.872 / 194.304 | 77.257 / 98.203 / 341.127 |
| Payload p50 (ms) | 147.961 | 133.590 |
| Root wait p50 (ms) | 57.541 | 46.551 |
| Cumulative saves including drain (s) | 48.546 | 579.187 |

RocksDB engine CPU was 77.948 ms/block and context switches were 1943.668/block
in the 400-block counter window, versus 77.673 ms and 1841.445 switches for the
gated-worker control. Direct slots did not materially improve RocksDB execution.
The execution median gap remains 2.559 ms, while saving time is 91.6% lower.
This form was rejected; artifacts and source patches are retained under
globalslots backend names with early0-proof64-chunk80-600. A separate candidate
will notify only when publishing the transaction that the consumer is waiting
for, with setup failures always notifying.
