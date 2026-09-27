# RocksDB state-trie optimization

Measured on dev-brian on 2026-09-26–27, continuing the
[latency investigation](state-trie-rocksdb-latency.md).
Artifacts, captured binaries/source patches, and reproduction scripts are in
`/home/ubuntu/state-trie-optimization-20260926`. The full experiment notebook
is retained there as `experiment-notebook.md`.

## Outcome so far

The strict execution target remains open. The latest matched, uninstrumented
runs use proof chunks of 80 and the same retained code and CPU settings.
RocksDB execution p50 is 95.36 ms versus 87.78 ms for MDBX, with cumulative
saves of 51.51 s versus 542.59 s (90.5% lower). Payload p50 is 117.26 versus
109.87 ms. RocksDB has lower execution and payload p99 in this pair.

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
- Honor existing runtime worker-count CLI settings in `reth-bb`, and allow
  shutdown enough time to flush the large-block workload.

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
