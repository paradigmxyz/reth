# newPayload pipeline: parallel execution with an ordered committer

Status: in progress. Phase 1 is on branch `mattsse/parallel-newpayload`
([paradigmxyz/reth#28293](https://github.com/paradigmxyz/reth/pull/28293)).

## Goal

Validate an Ethereum mainnet payload about three times faster than reth `main`: from about
1.1 Ggas/s and a 25-34 ms mean `engine_newPayload` (derek, 500 mainnet blocks) to about 3 Ggas/s
and 10 ms. Nethermind is 20-30% faster than reth on mainnet today, so tuning the existing
sequential path is not enough.

## Where the time goes

Measured on recent mainnet blocks (26,327,216-26,327,715, about 28 Mgas and 340 transactions per
block) with `examples/exec-replay`. It records every parent-state value a block reads from a
read-only archive datadir and replays the block in memory, so execution can be timed and compared
without state access.

| Measurement | Mean per block |
|---|---|
| derek `newPayload`, reth `main` | 25-34 ms |
| Sequential execution in memory, precompile cache warm | 9.4-9.7 ms (2.9-3.3 Ggas/s) |
| Sequential execution in memory, precompile cache cold | 12.1 ms |
| Ordered commit of ready speculative results, in memory | 1.2 ms |

Two conclusions:

1. Even with every account and slot in memory, executing a mainnet block sequentially takes
   about 10 ms. A `perf` profile of that run puts 56% in the evm2 interpreter's opcode handlers,
   and the rest in state bookkeeping, hashing and commit (`BlockState` transition maps, the evm2
   overlay, keccak). Removing overhead around the interpreter cannot reach 3x, so the engine
   thread must stop executing most transactions itself.
2. The other 15-25 ms of `newPayload` are state access on the engine thread (cross-block cache
   lookups and MDBX reads that prewarming did not get to first), the state root after the last
   transaction, and fixed costs. Parallel execution also removes the state reads from the
   engine thread, because speculative workers do them.

## Where the node spends its CPU

A derek samply profile of `main` (500 mainnet blocks plus 125 warmup blocks, a 14-16 core box)
accounts for 159 s of CPU in about 23 s, so about 7 cores on average, in bursts per block:

| Threads | Count | CPU per block | What they do |
|---|---|---|---|
| `engine` | 1 | ~20 ms | Executes transactions serially: 92% of its samples; 27% of those are state reads through `CachedStateProvider` and the in-memory overlay, with page faults and TLB shootdown interrupts; 5% waits for the state root |
| `proof-acct`, `proof-strg` | 56 + 56 | ~57 ms | Multiproofs for the sparse trie: 71% inside MDBX trie cursors, 20% `page_get`, 19% page faults |
| `rayon` (global) | 14 | ~32 ms | Sparse trie storage passes and subtrie hashing |
| `prewarm` | 13 | ~32 ms | Speculative execution that warms the caches |
| `tokio-rt` | 20 | ~20 ms | Engine API, JSON decoding, history index writes |
| `cpu`, `storage`, `persistence`, `mdbx-rs-txn-mgr` | 16 + 17 + 1 + 1 | ~40 ms | Conversion, receipts, persistence |

Persistence interferes on top of that: derek runs from paradigmxyz/reth#27900 that keep all 500
blocks in memory (`--engine.persistence-threshold 1000`) were 19% faster on mean latency
(27.7 -> 22.4 ms) and 20% higher in Mgas/s. Writing blocks back through MDBX's writable map makes
the kernel write-protect dirty pages, which interrupts every core running a thread of the process
to flush its TLB, and the writer competes for cores and the page cache with the readers.

So a 32 ms block costs about 250 ms of CPU on about 300 threads. The engine thread is the
critical path, and most of the rest re-reads the state trie from MDBX for every block. When
execution leaves the engine thread, the cores it needs are taken by that work: on the Phase 1
branch, `prewarm-txs` took 24% longer and the engine thread's involuntary context switches rose
4.5x with the state root enabled.

## What the node showed

A local replay on the devbox (`ubuntu@dev-mattsse`, 16 cores with SMT, two CCDs) runs a mainnet
node from a reth snapshot, extracts 500 recent blocks from it, unwinds, and replays them through
the Engine API with `bench send-blocks`; stats exclude the first 100 blocks. main takes
16.4-17.1 ms mean, 15.5-15.7 ms p50.

A per-transaction timeline of the branch (`RETH_NP_TIMELINE` on an experiment branch) gave the
committer's waits. In the first version the committer finished at 14 ms, 9 ms of it waiting, and a
first speculative execution took 240 us on average, against 31 us for the same code in memory.
Fixing what the profiles showed, in order:

| Finding | Evidence | Change | Effect |
|---|---|---|---|
| Idle workers rescanned the refresh window on every stored result | 35% of worker time in `needs_refresh` (derek samply) | Readers register per key; changed predictions flag the later readers | derek +3.8% Mgas/s |
| Every stored result woke all parked workers, then a wake per flagged reader under map and slot locks | 15% kernel scheduler time in workers; 74 ms mean with 32 workers | Wake only for queued transactions and flagged results, once per update | 32 workers: 74 -> 21 ms |
| Too many speculation workers for the cores | 8-12 workers 14 ms vs 32 workers 16-17 ms locally; on derek the default 13 is best (6 and 8 are worse) | none yet: the best count follows physical cores | |
| First inserts into the execution cache tables took copy-on-write faults from the zero page | 98% of the TLB shootdown IPIs sent by workers came from `wp_page_copy` in `CachedStateProvider`, 10% of worker time | Map the tables in the cache, huge pages, populate writable in the background | spec 150 -> 124 us |
| A heavy transaction's speculation takes milliseconds while the committer waits | single waits of 2-6 ms in the timeline | Execute it on the committer after 300 us | locally -4% mean |
| Cold reads inside a transaction are serial | 21% of accounts and 29% of slots per block were not read in the previous 400 blocks | Probe each transaction against the cache, fetch its misses on 128 I/O threads | spec 124 -> 107 us, not faster overall: probing costs worker time |

Locally the branch now takes 12.5 ms mean, 11.5 ms p50 against 16.6-17.1 ms for main (-25%),
with 8 workers. On derek (6 run pairs, default workers) it is -5.8% mean, -13.0% p50 and +14.6%
Mgas/s against main; the persistence wait rose from 0.05 to 1.3 ms, so end-to-end throughput is
flat (+1.8%, within noise).

What still costs: a speculation is 3-4x the in-memory time. Of the workers' time, the balance
inspector's per-opcode `step` and per-frame `initialize_interp` are ~15% (and keep the interpreter
off its fast dispatch loop), predicted-state lookups 11%, the cache and overlay layers 12%, MDBX
10%, precompiles 9%. Persistence interference is 19% on derek (all 500 blocks kept in memory) but
4% locally; disabling MDBX's write map gains 2% locally.

### Phase timestamps

Per-phase timestamps of `newPayload` (`RETH_NP_PHASES` on the experiment branch
`mattsse/np-phases`) and the committer timeline then showed where the rest went:

| Finding | Evidence | Change | Effect (local mean) |
|---|---|---|---|
| A payload arrived while its parent's prewarm task still held the execution cache, so the lookup missed and allocated a new 4 GB cache | Two cache creations per run; one block per run stalled 3 s, its 81st transaction arriving at 3.16 s; mean 23 ms against a 13.5 ms p50 | Wait up to 50 ms for the parent's save; populate and unmap the tables in 64 MB chunks | 23 -> 14.4 ms |
| Pools sized to logical CPUs on an SMT machine | 32 prewarm workers and 32 + 32 rayon threads on 16 cores; 9% of the node's CPU in rayon work stealing; `--cpu-cores 12` with 4 proof workers: 11.1 ms | Size the cpu, global rayon, prewarm and BAL pools by physical cores | 14.4 -> 11.6 ms |
| Transactions converted in windows that each waited for their slowest transaction | The last transaction reached speculation 3 ms into a block | Converters claim transactions in block order and hand each to prewarming as soon as it is converted | last arrival 1.8 -> 1.2 ms |
| Proof pools sized to the cores starve the root on derek | derek (16 cores, SMT off): state root after execution 1.27 -> 5.65 ms, proof workers no longer idle | Keep four proof workers per cpu pool thread | |

Against main locally: 11.4-11.8 ms mean against 15.5-15.8 ms (-26%), 10.4-10.9 ms p50 against
14.6-14.8 ms, +34% Mgas/s. On derek (6 run pairs, 1410b257d2): mean 29.35 -> 26.01 ms (-11.4%), p50
-12.4%, p90 -6.7%, p99 -16.9%, Mgas/s +16.9%, end-to-end Mgas/s +6.1%. The state root after
execution is 2.62 ms there against 1.32 ms on main, and the engine thread is preempted 18.6 times
per block against 9.7: derek has no SMT, so the pools compete with it for the 16 CPUs.

Then, on the data structures and threads around execution:

- **Precompile cache.** On the committer, 78% of ecPairing and 49% of ecMul calls missed the
  moka cache, mostly because it executed a raced transaction while the prewarm worker was still
  computing the same calls, and moka lookups were 2.5% of the committer's own executions. The
  cache is now a fixed-size table probed without locks, and a call whose input is being computed
  waits for that output (single flight): committer misses of both halved.
- **Prediction entries.** Prediction bookkeeping was ~10% of speculation, a third of it
  registering readers in lists kept apart from the writes. Each key's writes, clears and readers
  now share one entry, a transaction registers as a slot reader of a contract once, and slot
  reads look at clears only after a predicted clear.
- **Priorities.** Proof workers and the global rayon pool run 10 nice levels below execution
  (from paradigmxyz/reth#28298): derek -2.5% mean, +3.5% Mgas/s on top of the branch, engine
  preemptions -22%.

derek, 6 run pairs against main, with everything above and the cold read probe below
(0bc00b7e02):

| | main | branch | Change |
|---|---|---|---|
| Mean | 27.77 ms | 22.85 ms | -17.7% |
| P50 | 22.57 ms | 18.87 ms | -16.4% |
| P90 | 45.79 ms | 37.96 ms | -17.1% |
| P99 | 127.70 ms | 98.95 ms | -22.5% |
| Mgas/s | 1,247 | 1,504 | +20.6% |
| End-to-end Mgas/s | 951 | 1,034 | +8.8% |
| Engine thread CPU per block | 14.20 ms | 7.44 ms | -47.6% |
| State root after execution | 1.13 ms | 2.80 ms | +149% |
| Engine thread preemptions per block | 9.8 | 22.1 | +126% |
| Persistence wait | 0.85 ms | 3.26 ms | +282% |

derek drops the page cache before every run (125 GB of memory), so its blocks wait on disk reads
much more than the local replays, whose page cache stays warm: main takes 29 ms there against
15.5 ms locally. What did not help on derek, each 4 run pairs against the branch:

| Experiment | Mean | Mgas/s |
|---|---|---|
| Sparse trie applies streamed updates every 16 messages (paradigmxyz/reth#28297) | +1.2% | -1.3% |
| Global rayon pool halved | -0.2% | +1.5% |
| Storage trie jobs on a pool that blocks instead of rayon's spinning workers | +0.1% | -1.0% |
| Sparse trie never pruned (`--engine.disable-sparse-trie-cache-pruning`) | +0.0% | -0.7% |

With the rayon and proof pools below execution priority, their spinning no longer costs
execution time.

What did help there is the probe of cold reads that made no difference locally: speculation
workers execute each arriving transaction against the execution cache alone, record what it
misses, and the read-set pool's I/O threads load those keys, so a block's cold reads are loaded in
parallel instead of one dependent read at a time. Against the branch without it: mean -5.1%, p90
-11.0%, p99 -13.0%, p50 unchanged. It is on with reuse.

The root after the last transaction (local p50): the hashing task has seen every update after
0.29 ms, most of which is the post-execution changes, the sparse trie after 0.46 ms, and the root
is ready after 1.46 ms. So about 1 ms is trie work after the last update, not a backlog of
updates.

With pools sized, a block is 8.5 ms in the transaction loop (p50), 1.1 ms of state root after it
and 0.8 ms of everything else. Of the committer's 9.3 ms in the loop, 6.3 ms is executing about 27
transactions itself, nearly all in a handful of heavy ones:

| Committer cost per block | Count | Time |
|---|---|---|
| Heavy transaction whose first speculation is still running when the committer reaches it (raced after 300 us) | 2.0 | 3.6 ms |
| Heavy stale result | 6.6 | 3.0 ms |
| Light stale result | 15.8 | 0.45 ms |
| Waiting for and committing reused results | 265 | 2.5 ms |

The stale results read a storage value an earlier speculation predicted and then wrote differently
(9.5 per block, 1.9 ms), storage an earlier transaction wrote without predicting it (2.8, 0.9 ms),
or are unusable (10.4, 0.5 ms). They are chains: the earlier transaction was stale itself. What did
not help, locally:

- A refresh window of 512 instead of 64 transactions: committer -3%, within run-to-run noise.
- Racing after 3 ms instead of 300 us: the time moves from executing into waiting.
- `--jit`: no change on the branch or on main.
- Running the workers at a lower priority than the engine thread (`renice`): no change.
- Probing for cold reads: no change. Speculation workers take 13 major faults per block; MDBX
  reads, page faults included, are ~20% of their time.

### The p99 is a handful of cold batch transactions

derek's per-block results show the p99 is set by the same few blocks in every run, not by
random stalls. Replaying derek's exact 625 payloads on the devbox from the same snapshot, with
the datadir dropped from the page cache before each run (`ab3.sh` with `COLD=1`), reproduces
derek: main 26.2 ms mean, 102 ms p99, 24 blocks over 64 ms; the branch then 12.

Each of those blocks has one transaction that takes 70-200 ms cold and 1-10 ms warm, in
speculation and on the committer alike: batch calls with 7-14M gas limits and 4-19 KB of
calldata (a Safe `execTransaction`, a bot contract seen in two of them), whose storage reads go
to disk one at a time. Warm, the same 500 blocks have a 28 ms p99.

The probe found few of their keys, for two reasons: a missing slot read as zero, so the batch
reverted at its first balance check, and it ran once. Probe threads now probe every arriving
transaction in up to 64 rounds. Each round loads everything the previous one missed in parallel
on the read-set pool and waits for it, then executes again; missing slots read as 2^128 in every
other round, so that balance and allowance checks pass. Speculation workers no longer probe, so
the probes' waits do not delay speculation. Cold, over several runs each:

| | Mean | P99 | Blocks over 64 ms |
|---|---|---|---|
| main | 26.44 ms | 105.7 ms | 21-24 |
| branch, one probe round on the workers | 18.75 ms | 96.5 ms | 9-13 |
| branch, 64 rounds on probe threads | 16.51 ms | 69.6 ms | 4-6 |

Rounds load thousands of keys at once for some of these transactions (26342981: 2,871 and 2,539
keys, 79 -> 47 ms). Two remain: 26342682 and 26342631 walk a chain of dependent reads, one or
two keys per round, for 64 rounds and beyond (26342682 through a contract with 1.77M slots), which
no prefetch can parallelize. Loading rounds of up to 8 keys on the probe thread instead of the
pool, and code with every account, made it slower (p99 74.7 ms).

On derek, durable commits of persistence batches cost the rest: running the same branch with
`--db.sync-mode=safe-no-sync` gave mean -11.7%, p99 -23.2% and end-to-end Mgas/s +24.5%. In that
run the chain transactions' blocks are as fast as on main and stable across runs; with durable
commits they take up to twice as long in the runs where a commit's flush lands on them. Opening
MDBX without its write map is not an option: the node reports the database as corrupted.

Further changes and experiments on derek (4 run pairs against the branch unless noted):

| Change | Mean | P99 | Kept |
|---|---|---|---|
| Probe loads on a quarter of the read-set pool (32 workers), so fewer of them queue on the disk ahead of a chain's own reads | +0.0% | -10.4% | yes |
| Flatten the overlay from the next anchor when a save starts, instead of rebuilding it from every in-memory block when it completes (5-9 ms, one block in twenty; local cold: mean -3%, p90 -6%) | | | yes |
| Writeback of dirty pages every 20 ms during a save (`sync_file_range`) | +12.0% | +6.9% | no |
| `--engine.persistence-threshold=128` | +5.3% | -2.7% | no |
| Probe the first transaction too (local cold) | +8% | | no |

So with durable commits, the remaining p99 is the persistence flush meeting a block's serial
cold reads. Spreading the flush out slowed every block more than it helped the worst ones.

`--db.sync-mode=safe-no-sync` already bounds what a crash loses: the first commit 60 s after the
last flush flushes. `--db.sync-interval` sets that time. Flushing from a background thread
instead of the commit is not possible with `mdbx_env_sync`: with `MDBX_NOSTICKYTHREADS` it takes a
write transaction open on another thread as its caller's, skips the write lock, and the node
aborted on an MDBX assertion during a commit. Writing the data file back with `fdatasync` from a
thread does work, but made no difference locally (mean 17.05 ms durable, 17.46 ms with a 1 s
interval, 17.95 ms with the thread): on the devbox's NVMe, commits take ~105 ms in every mode, so
their cost there is not the flush. On derek, against main (4 run pairs each, with the change
below):

| | Mean | P99 | End-to-end Mgas/s | Persist wait |
|---|---|---|---|---|
| Durable | -22.7% | -41.3% (143 -> 84 ms) | +9.5% | 0.62 -> 4.47 ms |
| `safe-no-sync`, `--db.sync-interval=12s` | -23.8% | -42.0% (133 -> 77 ms) | +30.6% | 0.76 -> 0.09 ms |

With durable commits, persistence cannot keep up with the faster blocks: each save of ~18 blocks
holds its write transaction for ~370 ms, and the benchmark waits for it.

### Where block execution waits on a cold page cache

Per-block counters of block execution (the committer) on the cold replay, 1,000 blocks:

| Committer work per block | Count | Time |
|---|---|---|
| Executing transactions whose first speculation was still running after 300 us (raced) | 2.9 | 6.6 ms |
| Spinning before racing | | 1.1 ms |
| Re-executing stale results | 30.4 | 2.8 ms |
| Executing transactions no worker had started | 3.0 | 1.4 ms |
| Committing reused results | 270 | 1.7 ms |

Almost every transaction is probed before block execution reaches it (303 of 306 per block, 1.2
rounds each), so the probe keeps up. The slow blocks are one to four raced transactions each:
26342682 240 ms on four, 26342631 175 ms on three, 26342742 83 ms on one, and 26343102 72 ms on
its first transaction, which block execution executes itself. Racing executed the same chain of
cold reads a second time beside the worker, and both waited on the disk. Waiting for the worker
instead (local cold, 2 runs each):

| Raced transactions | Mean | P90 | P99 | Root after execution |
|---|---|---|---|---|
| Executed by block execution after 300 us | 17.82 ms | 27.05 ms | 73.3 ms | 2.32 ms |
| Waited for | 16.74 ms | 24.52 ms | 66.1 ms | 1.95 ms |
| Waited for while their probe runs | 17.74 ms | 26.26 ms | 74.1 ms | 2.22 ms |

Warm, waiting made no difference (11.35 vs 11.38 ms). Block execution now waits, and a guard
clears the speculation flag so that a panicking worker cannot leave it waiting.

The probes of the remaining slow transactions show three limits:

- 64 rounds were not enough: 26342742's fifth transaction still found 38 keys a round when the cap
  stopped it after 20 ms, and the worker then read the rest one at a time for another 50 ms.
- A round re-executes the transaction from the start, ~0.85 ms for the heaviest ones, more than
  a cold read: on pure chains (26343120's sixth transaction, 4 keys a round for 48 rounds) the
  probe is no faster than the worker.
- A probe whose miss cannot be loaded (code missing from the database) misses it again every
  round until the cap.

Raising the cap to 256 rounds, spreading small rounds one slot per load request, loading code
with every probed account, skipping keys a probe already loaded, probing the first transaction
and running probes at normal priority were each within the ±5% noise of the cold replay over 4
runs per arm.

### Branch pages on a cold page cache

Classifying the major faults of a cold replay by the MDBX page they hit (fault address, then the
page header flags in `mdbx.dat`): 29-30% of execution's and 18-22% of the proof workers' faults
are branch pages. Reading every page flagged as a branch page (4.55M, freelist included) before
the replay: mean 17.17 -> 14.90 ms, p99 75 -> 63 ms, the chain blocks 150-180 -> ~110 ms.

`--db.warm-branch-pages` reads them at startup by walking the B-trees from the table roots in
the main table, level by level on 64 threads. The tables' own trees hold few of them (238K pages,
0.6 s, no effect): a duplicate-sort table keeps a large set (a contract's storage, an account's
storage trie) in a tree of its own that only a leaf points to. Reading the leaves of
`HashedStorages`' and `StoragesTrie`'s trees (2.9M pages) and the nested trees' branch pages
brings it to 5M pages (20 GB) in 9-11 s. Read beside block execution it slowed the first blocks
(cold replay mean 17.0 -> 20.8 ms in the run it overlapped), so startup waits for it.

This mainly helps a restarted node and a benchmark that drops the page cache: on a node that has
run for a while, branch pages are touched far more often than leaves and stay cached.

## The request path around execution

Per-thread CPU over a warm replay (`threadcpu.py`, from `/proc/<pid>/task`), per block:

| Threads | CPU before | After | Involuntary switches before | After |
|---|---|---|---|---|
| Speculation workers | 65 ms | 64 ms | 169 | 137 |
| Global rayon pool | 35 ms | 12 ms | 2,731 | 252 |
| Proof workers (storage + account) | 41 ms | 25 ms | 3,222 | 425 |
| Storage trie pool | - | 11 ms | - | 20 |
| Sparse-trie thread | 9.8 ms | 7.7 ms | 7 | 6 |
| Total | 211 ms | 180 ms | | |

The state root used more CPU than execution, and a third of it was idle threads looking for
work: 73% of the global rayon pool's samples were in `wait_until_cold` (work stealing,
crossbeam-epoch collection, `sched_yield`), and so were a third of the proof pools'. A block
claims a quarter to all of the proof pool's 4 x cores threads, and rayon's unclaimed threads spun
while the claimed ones blocked on their channels; ~450 storage trie passes per block were each a
`rayon::spawn` that woke threads to spin. These ran at the priority of the trie work itself.

- Proof workers run on `BlockingWorkerPool`, plain threads that wait on a queue.
- Storage trie passes run on the runtime's storage trie pool of the same kind.
- The root took updates from every retained storage trie (4,500) and preserving the trie
  collected every retained trie's root into a new map, 12% of the sparse-trie thread and before
  the next block's task can start. Both now visit the tries the block changed; the previous
  block's root map is copied and updated, and a debug assertion compares it with a full pass.
  `root_with_updates` 733 -> 251 us.
- Removing persisted blocks dropped their execution output on the engine thread between
  payloads (3% of it); they are now freed on the drop thread.

Local replay, the first three changes against the commit before (2 run pairs each):

| | Before | After | Change |
|---|---|---|---|
| Warm mean | 11.52 ms | 10.70 ms | -7.1% |
| Cold mean | 18.06 ms | 15.81 ms | -12.5% |
| Cold p90 | 27.5 ms | 23.6 ms | -14.4% |

The storage trie pool on its own was within noise for latency (warm -2.4%) and saves 25 ms of CPU
per block.

derek, 4 run pairs against main:

| | ebc6075b51 | ebc6075b51, `--db.warm-branch-pages` | 61e6963752, warm-up and `--db.sync-interval=12s` |
|---|---|---|---|
| Mean | -25.4% (27.68 -> 20.65 ms) | -35.3% (28.85 -> 18.66 ms) | -37.3% (29.02 -> 18.19 ms) |
| P50 | -21.7% | -29.5% | -29.4% |
| P90 | -28.4% | -42.7% | -44.9% |
| P99 | -43.4% (145.6 -> 82.4 ms) | -52.3% (139.3 -> 66.4 ms) | -63.3% (162.0 -> 59.5 ms) |
| Mgas/s | +27.3% | +43.7% | +44.8% |
| End-to-end Mgas/s | +11.2% | +14.9% | +14.0% |

With the probe stop and freeing persisted blocks off the engine thread (6c83d74744): mean -26.2%
(27.42 -> 20.25 ms), p90 -32.8%, p99 -32.7% (127.6 -> 85.8 ms), Mgas/s +30.9%; with
`--db.warm-branch-pages` mean -36.7% (28.88 -> 18.29 ms), p90 -41.4%, p99 -56.8% (157.8 ->
68.1 ms), Mgas/s +45.9%. Locally the probe stop was -3.8% warm and -6.9% cold.

main's own p99 ranges from 128 to 162 ms across these runs. Persistence does not keep up with the
faster blocks even without durable commits (persist wait 4.7-7.7 ms against 0.5-1.0 ms on main):
each save of ~18 blocks holds its write transaction for ~370 ms.

### Chains through one contract's storage

With the branch pages read at startup, the cold replay's p99 (75 ms locally, 68 ms on derek) is
set by about ten blocks whose heaviest transaction walks a chain of dependent cold reads: through
one contract's storage (26342682 through a contract with 1.77M slots, 26343111 and 26343120
through one with a few hundred keys a block, 26342888, 26343115), through accounts and code of
dozens of contracts (26342631, 26342690), or as the first transaction, which block execution
executes without a probe (26343102). Probes cannot get ahead of a chain: every round re-executes
the transaction up to its next miss (0.5-2.5 ms for the heaviest), slower than the speculation's
own serial cold reads (~100 us), which already loaded most of what a round asks for.

A probe that misses at most 8 slots, at least half in one contract, four rounds in a row now
bulk-loads that contract's storage: 64 duplicate cursors walk the slot space in equal ranges on
background threads, up to 2M slots, at most once a minute per contract. 148 loads in the 625
blocks, 5-114 ms each. Cold, with the branch pages read (2 runs each): p99 74.4 -> 49.4 ms,
26342682 117-152 -> 14 ms, 26343120 57 -> 33-47 ms, 26342742 69 -> 51 ms; mean 14.87 -> 15.06 ms
and p90 22.2 -> 24.5 ms, from the loads' reads. A 512K-slot cap left the 1.77M-slot contract
unloaded (26342682 back to 90-100 ms), and waiting for 8 rounds lost 26342742.

Probing the first transaction too, so that its chains are loaded as well, made 26343102 slower
(52-58 -> 70-88 ms). On derek, 4 run pairs against main, with `--db.warm-branch-pages`
(3bd132c98a): mean -34.0% (29.19 -> 19.26 ms), p90 -35.9%, p99 -65.2% (165.9 -> 57.8 ms),
Mgas/s +38.3%.

What is left is the transactions' own chains of cold reads through accounts, code and storage,
and their execution: warm, the slowest block (26342631, one transaction of ~25 ms) takes 25-40 ms
and sets the warm p99 of ~30 ms, which is about 20% of main's cold p99. Reading the whole
`HashedAccounts` (8.6M pages, 14 s) and `Bytecodes` (5.5M pages with overflow, 9 s) tables at
startup as well (experiment branch `mattsse/np-warm-tables`, 73 GB with the branch pages; it does
not fit the devbox's page cache): mean -37.4% (27.39 -> 17.14 ms), p90 -40.7%, p99 -66.0%
(138.3 -> 47.1 ms), Mgas/s +45.8%, against a persist wait of 9.8 ms.

### The root after execution

Warm, the root is ready 1.27 ms after post-execution validation (was 1.76 ms): ~0.6 ms of trie
work after the finish marker, 0.25 ms for the root and updates, and the rest the marker's way
there (it is sent when evm2's executor drops its state hook, after building the bundle state,
~0.25 ms on the engine thread). In 45% of blocks the trie requests proofs after the finish marker,
almost all for one to three USDT or USDC slots under a blinded node, and those blocks' tail is
1.57 ms against 0.67 ms. The task applies buffered updates, prefetch hints included, only when its
queue runs empty, so in a continuous stream a hinted slot's proof is requested at the end.
Applying all buffered updates every 4 messages halved those blocks but made the others' tail
1.6 ms (the task fell behind), mean +5%; every 16 messages was neutral. The late targets are not
the hints waiting behind updates: buffering hints apart and applying only them on every message
left as many blocks with late targets. Nor are they keys nobody hinted: also touching every
updated key on arrival made more of them (the task fell behind, mean +16%). They are full keys
under a known parent, requested when a value is applied, which points at leaves that only a
change reaches, such as a removal collapsing a branch onto a blinded sibling.

Counting the trie's proof requests after the finish marker by reason confirms it: a removal that
leaves its branch with one blinded child, in 159 of 500 blocks (blinded writes in 51, hints in 33).
Checking each removed slot on arrival, without applying it, and requesting the sibling its removal
would need (a read-only `prefetch_removals` on the trie) did not reduce them (171 blocks), alone or
with hints applied eagerly (204-209), and the extra passes made the cold replay 4-10% slower. At
arrival the leaf's path is usually not revealed yet, and the structure that decides a collapse is
the one after the whole last batch, in which several removals can empty a branch together.

### What the next block's state root waits for

A per-event trace of the sparse-trie and trie-hashing threads (experiment branch
`mattsse/np-phases4`, written to a file so that journald drops nothing) shows the hashing thread
reading a block's first update more than 0.5 ms late in 43 of 500 warm blocks, with the block's
updates queued behind it, and those blocks' root tail at 4.7 ms against 2.1 ms. Both threads are
named threads that run one task at a time, and in 17 of these blocks the task waited for the
previous block's:

- In 14 blocks the sparse-trie thread was still pruning the previous block's trie, which runs
  after every persisted block, after the root is sent and before the trie is handed on. A prune
  took 15-21 ms for the account trie (25-33k nodes pruned) and 8-10 ms beside it for the storage
  tries (3,400-4,600 tries, 500-1,300 evicted), with about three threads busy on average. Running
  it on the normal-priority cpu pool instead of the global rayon pool made no difference, so it
  is the work, not starvation. Pruning a sixteenth of the trie per block instead
  (one first nibble of the account paths and of the storage tries' hashed addresses) cost 1.3 ms
  p50 and 7.6 ms p99 per part, 0.49 s per replay against 0.29 s, in 40% of the blocks, and the
  mean got 6.6% worse over 4 run pairs. A late start costs the delayed block little because its
  task catches up with the queued updates during execution, about 0.07 ms per block on average,
  so the prune stays as it is.
- In 3 blocks the hashing thread was still running the previous block's hashing task, which
  waited for every update sender to drop and prewarming holds a hint sender until its workers
  stop. It now returns once it has forwarded the finish marker (up to 0.7 ms).

### The sparse-trie thread after the last transaction

The same trace shows what the sparse-trie thread blocks on between the last transaction and the
root, per block on average: hashing the account trie's dirty subtries (`calculate_subtries`) 479 us,
storage passes run inline 313 us, promoting account updates 171 us, applying updates 47 us.
Meanwhile prewarming kept its workers running until 0.89 ms after the last transaction. The
finish marker and proof results queue behind these calls: in the median block (26343110) one
`calculate_subtries` took 468 us while the marker waited, and one inline pass hashed a storage
trie for 418 us.

- A storage pass hashed its trie only once no leaf waited for a proof, so a trie with one blocked
  leaf accumulated every leaf applied to it and hashed them all in the pass after the last proof,
  often inline because the pass itself was small. Passes now hash what they applied either way:
  inline passes after the last transaction 313 -> 88 us.
- Prewarming now stops once block execution has executed the last transaction instead of after
  the post-execution changes: its workers end 0.54 ms after it instead of 0.89 ms.
- The account trie hashed its dirty subtries serially below 64 dirty leaves, at about 10 us per
  leaf: 32-63 dirty leaves took 415 us p50 after the last transaction, more than 64-127 in parallel
  (237 us). From four dirty leaves on they are hashed in parallel: 163 us for 32-63, 91 us instead
  of 164 us for 8-31, and the root's tail p50 -8%. From 128 dirty leaves on a call still takes about
  430 us: parallel hashing scales to about 4x.

The first two made the root come 23% sooner after the last transaction (p50 1.33 -> 1.03 ms, 4
run pairs). On derek, with the hashing task change, against the head before them and
`--db.warm-branch-pages` on both sides (e11befe730): mean -1.15% (18.45 -> 18.23 ms, noise
±1.43%), p50 -0.4%, p90 +1.9%, p99 +5.3% (noise ±9.9%), none significant. With all four
(d16ffadedb) against main, `--db.warm-branch-pages` on the branch: mean -32.4% (27.61 -> 18.67 ms),
p50 -27.5%, p90 -34.4%, p99 -51.6% (125.4 -> 60.7 ms), Mgas/s +40.7%, end-to-end Mgas/s +8.6%;
the branch's mean was 19.26 ms at 3bd132c98a, and main's p99 ranges from 125 to 166 ms across runs.

### What the state root costs

Replays with `--debug.skip-state-root`, which runs no state root job and installs no state hook,
bound what the state root can still win. 3 runs each, the phase timestamps written to a file
(journald dropped up to two thirds of them under load, which made earlier comparisons of the
execution phase misleading):

| | Default | No prefetch hints | Skip state root |
|---|---|---|---|
| Transactions phase, mean | 11.86 ms | 12.34 ms | 11.28 ms |
| Transactions phase, p50 | 9.99 ms | 10.39 ms | 9.32 ms |
| Root after the last transaction, mean | 1.49 ms | 3.06 ms | 0.43 ms |
| newPayload, mean | 13.73 ms | 15.83 ms | 12.53 ms |

The state root costs 1.2 ms of a 13.7 ms block: about 1.1 ms after the last transaction and
0.6 ms during execution. Committing reused results took 320 us longer per block with the state
hook installed, which builds a `StateUpdate` for every transaction and sends it to the hashing
thread on the committer. The send is 149 us of it, mostly waking the parked hashing thread;
spinning that thread for 50 us before it parks cut the send to 64 us but made execution no faster,
and building the updates is evm2's. Proof prefetching is not what slows execution: dropping the hints moves
the proofs after the last transaction and slows execution slightly, and 16 or 32 workers per proof
pool instead of 64 changed the transactions phase by less than 2% while the tail grew.

What is left after the last transaction is mostly outside the trie: the finish marker is sent when
evm2's executor drops its state hook, after it built the bundle state, ~0.3 ms after the last
transaction. Dropping the hook right after the post-execution changes is a change in evm2. In the
trie, the walk that restores the hashed account subtries and merges their trie updates into the
upper trie's is 35-40% of a large `calculate_subtries` (159 of 446 us with 128-255 dirty leaves),
serial on the sparse-trie thread.

Patching evm2 locally to drop the hook right after the post-execution changes (experiment branch
`mattsse/np-phases8`, behind an environment knob) moved the finish marker from 341 to 50 us after
the last transaction (p50) and made the replay faster in all 3 run pairs: mean 15.85 -> 15.09 ms
(-4.8%), p50 -5.4%, p99 -8%. The root itself came only 74 us sooner: with the marker early, the
tail is the trie's own work on the last updates. It needs a change in evm2.

### Proofs are reads from disk

Timing every proof job (dispatch to start, and compute) shows the slowest tails are single proofs:
in the worst blocks one job computes for 10-32 ms, warm as well as cold, and the root follows it.
Storage proof jobs take 190 us p50, 0.9 ms p90 and 2.9 ms p99, account proof jobs 660 us p50 and
4.6 ms p99, about 375 jobs per block. The devbox's page cache (46 GB for a 279 GB database) does not
hold the trie tables' leaf pages, and the proof workers took 730k major faults over a "warm" replay,
about 1,170 per block, against 240k minor ones. On the CPU, 42% of the proof workers' time is MDBX
cursor searches, a third of it in the kernel under them, 13% computes storage roots for account
leaves whose root the reused trie does not know (siblings included, each a random read of their
storage trie) and 7% checks whether an account's storage is empty.

- Full proof pools (64 + 64 workers instead of 32 + 32) and one target per job instead of 5 cut the
  tail by 11% (p99 13.1 -> 11.2 ms) in 2 run pairs, within noise for the block.
- Never pruning the sparse trie left 18% fewer proof jobs (235 against 286 per block) and the
  tail's p99 at 5.0 ms instead of 14.3 ms; pruning only what was last changed more than 256 blocks
  ago, 244 jobs and 9.2 ms: the rest are nodes that were revealed and not changed, which every prune
  drops. Allocated memory grew from 1.9 to 2.2 GB (256 blocks) and 2.45 GB (never) over the 625
  blocks. On derek, never pruning against the head (both with `--db.warm-branch-pages`, b4381d3397):
  mean -0.8% (noise ±2.3%), p50 -2.7%, p99 +9.3% (noise ±11.4%), none significant, so the prune
  stays.

The root no longer sets derek's p99. The blocks there are slow in execution.

### Where block execution waits

Per-thread scheduler statistics (`/proc/<pid>/task/*/schedstat`) over a replay: the engine thread
that commits uses 13.8 ms of CPU per block and waits 0.2 ms for a core, so it is not preempted; the
machine is saturated (speculation workers 64 ms of CPU per block, proof workers 38 ms, the global
rayon pool 20 ms, each pool below execution's priority waiting 8-23 ms per block for cores). The
node's attempt to raise the engine and sparse-trie threads' priority fails without `CAP_SYS_NICE`
(both run at nice 0 on the devbox); granting it raises the speculation workers too, which inherit
the engine thread's priority.

A per-transaction timeline of speculation and the committer splits the transactions phase: the
committer waits 6.4 ms per block (p50 4.4 ms) for speculative first executions that are already
running when it reaches them, 1.8 ms of it for executions that waited more than 0.2 ms for a free
worker; it executes 25 stale results itself (2.8 ms) and 2.7 missing ones (1.5 ms); committing the
reused results takes ~1.2 ms. No wait is for a refresh of a stale result or for a transaction that
had not started. 3.7 ms of the wait falls on transactions after the 50th, 0.8 ms on the first five,
and the longest are single heavy transactions (26342631 index 42 and 26343111 index 2, ~80 ms each).

The waits are not scheduling gaps. A block's ~300 transactions reach speculation within 0.65 ms,
heavily out of order (63% before an earlier one), and a first execution starts 2.1 ms (p50) after
its transaction arrived even once every earlier one had arrived: the 17 workers are busy, many
blocked on reads from disk (406k major faults per replay). 32 workers instead of 16 cut the
committer's waits from 6.5 to 5.3 ms per block and its stale re-executions from 25 to 14, but the
transactions phase took 13.9 ms instead of 13.4 ms and the block 1.6% longer; 48 workers made it
15% slower. The machine is short of cores, not of workers.

Of the stale results whose reads fail validation, 12.5 per block fail on a storage read and 0.6 on
an account: token balances of USDC (3.4), USDT (3.2) and WETH (1.5) and Uniswap v4's pool manager
(1.3). In 6.8 the slot's last predicted writer is the previous transaction, a dependency chain
that refreshing cannot shorten; merging such results needs to know how the transaction used the
value, which the read sets do not record.

### The trie's CPU

With the cores the limit, CPU the trie does not spend goes to speculation. Per block, from the
threads' scheduler statistics over a replay: proof workers 38 ms, the global rayon pool 20.9 ms,
the storage trie pool 13.2 ms, the sparse-trie thread 9.1 ms, the trie-hashing thread 2.3 ms.

- 60% of the rayon pool's CPU was its threads looking for work after the account trie's parallel
  subtrie hashing (`wait_until_cold`: stealing, epoch collection, `sched_yield`), about 50 calls
  per block from four dirty leaves on. The arena trie now takes an executor for that hashing and
  the engine's tries hash on the storage trie pool, whose threads park when idle
  (`BlockingWorkerPool::run_all`, the calling thread taking jobs too). Rayon 20.9 -> 3.0 ms, the
  storage trie pool 13.2 -> 20.1 ms, the sparse-trie thread 9.1 -> 10.0 ms: 10 ms less per block,
  mean -2.4% over 3 run pairs (within noise).
- Proof workers took their jobs from crossbeam channels, whose receivers spin and yield before they
  park: 8% of their CPU. They now wait on a queue under a mutex: 40.2 -> 37.0 ms per block for
  both proof pools, 15% fewer context switches, mean -2.7% over 3 run pairs (within noise).
- Bigger proof jobs spend less CPU but finish later: `--engine.multiproof-chunk-size 20` instead
  of 5 cut both proof pools from 37.7 to 35.0 ms per block and raised the mean from 15.8 to
  16.7 ms over 2 runs each. The default stays.

derek, both changes against the commit before them, `--db.warm-branch-pages` on both sides, 4 run
pairs: mean 18.21 -> 17.58 ms (-3.5%), p50 -4.7%, Mgas/s +4.9%, the state root after execution
1.94 -> 1.29 ms (-34%), involuntary context switches of the engine thread -11%.
- Account proofs compute the storage root of every account leaf whose hash they need and whose
  root the reused trie does not know, siblings included: 2,045 per block, each a random read of the
  account's storage and storage trie, 13% of the proof workers' CPU plus 7% checking for empty
  storage. Only 21% of them were computed in an earlier block, so a cache across blocks would save
  little; the database does not store the hashes of leaf children.
- Hashing more often than needed is moderate: 270 storage trie hashes per block over 141 tries
  (8.8 ms), 585 account promotions over 520 accounts, 84 `calculate_subtries` calls (2.0 ms).

## Rewrite: newPayload v2

The v2 path replaces the per-block task graph instead of tuning it:

1. **One scheduler.** A fixed pool of workers, one per physical core and pinned, parked on a futex
   when idle, runs every task of the pipeline with priorities: the committer's work first, then
   the state root, then speculation, then prefetching. It replaces the proof worker pools, the
   prewarm pool and the global rayon pool on this path.
2. **Resident hot state.** A flat open-addressed table of accounts, slots and code on huge pages,
   owned by the engine at the canonical head. A read is one probe. Speculation's read sets drive
   batched cold reads. Committed blocks update it in place; it replaces the evm2 overlay,
   `CachedStateProvider`, the in-memory block overlay and MDBX on the read path.
3. **Execution on the hot state.** Speculative parallel execution with the ordered committer from
   Phase 1, writing the block's changes into a per-block delta of the hot state instead of
   per-transaction transition maps and state update copies. The bundle state and reverts for
   persistence are built once, off the critical path.
4. **Resident state trie.** Trie nodes of the account trie and of hot storage tries stay in
   memory across blocks with their hashes, so a block's root needs no proofs for resident paths.
   The block's hashed diff is applied per subtrie in parallel as speculation predicts it, and only
   leaves whose committed value differs are rewritten at the end.
5. **Persistence off the critical path.** With reads served from the hot state and the resident
   trie, the critical path touches MDBX only for cold misses, and persistence runs at a lower
   priority than the pipeline.
6. **Fallback.** Blocks the v2 path does not support (unknown parent, BAL blocks, reorgs past the
   hot state) run on the existing path, which stays the reference for differential tests.

## Phase 1: speculative parallel execution with an ordered committer

reth already speculates: prewarm workers execute every transaction against the parent state plus
the writes earlier speculative results predict, and record their reads. Block execution can commit
a result instead of executing the transaction when its reads still match the committed state,
rebasing balances the transaction only credited or debited (`payload_processor/reuse.rs`,
[#27894](https://github.com/paradigmxyz/reth/pull/27894)). On mainnet it gained about 7%.

### The rules are not the limit

The corpus test `corpus_reuse` speculates every transaction of a block in order, refreshes stale
results, then runs the commit loop:

| Mode, 100 blocks, 42,702 transactions | Reused | Commit loop |
|---|---|---|
| Speculation against the parent only | 64.6% | 5.9 ms |
| Plus one in-order refresh against predicted writes | 99.6% | 1.2 ms |
| Sequential execution | - | 9.4 ms |

Receipts and bundle state are identical in every mode. With ready results the engine thread does
7-8x less work.

### The scheduling was the limit

- Prewarming spawned one task per transaction and refreshed stale results only after all first
  executions finished.
- Refreshing polled with 50 us sleeps.
- Block execution never waited. It reached transactions whose results were still in flight,
  executed them itself with cold state, and fell behind the workers for the rest of the block.

The new scheduler (`prewarm.rs`, `speculate_txs`):

- Every prewarm pool thread runs one worker for the whole block. A worker claims the next queued
  transaction in block order. When none is queued, it executes again the first stale result among
  the 64 transactions after the one block execution is at. It rescans that window only after the
  committer moved or a result was stored.
- The committer (the engine thread) waits for a transaction whose first speculative execution is
  in flight. It executes a transaction itself only if no worker started it or its result is
  stale, and publishes the actual writes so that later results are refreshed against them.

### Hot keys

A profile of the committer under 16 workers had 14% in `dashmap` exclusive-lock contention: nearly
every transaction credits the block beneficiary, so every speculative result published a predicted
balance for one key, and every rebased reuse republished it. Predictions only steer speculation.
Correctness comes from validating reads against the committed state, so leaving a write out of
the predictions is always safe. Results no longer predict the beneficiary's balance-only writes,
and speculation sees the beneficiary as non-empty so its credit is rebased. Nonce changes are
still predicted, because builders send transactions from the beneficiary within the block, which
made 95% of stale results when everything about the beneficiary was left out. Reuse also
republishes only rebased account infos, not a result's whole write set.

### Results so far

In memory with real worker threads (`corpus_parallel`), all 500 corpus blocks (170,510
transactions), 8 workers, identical receipts and bundle state for every block:

| | Mean | p50 | p99 |
|---|---|---|---|
| Sequential | 10.5 ms | | |
| Parallel | 5.8 ms | 5.2 ms | 14.8 ms |

Worker counts on 200 blocks: 4 workers 6.2 ms, 6 and 8 workers 5.7 ms, 12 workers 6.2 ms. The
committer's 5.8 ms split into 1.7-1.8 ms waiting for first speculations, 2.4 ms executing the
27 stale results per block itself (mostly swaps on pools an earlier transaction in the block also
used), and about 1.6 ms of commit work. The stale ones form dependency chains that are serial
whoever executes them: waiting once for an in-flight refresh instead of executing cut the
committer's own executions to 1.3 ms but added the same time in waiting. So this design's
in-memory floor is about 1.75x for execution. Going lower needs cheaper chain links (a JIT, or a
faster interpreter) or merging the effects of common conflicts, as balance rebasing already does
for balances.

derek on mainnet, first run (idle workers spinning): mean 32.2 -> 38.0 ms (+18%), Mgas/s
1,183 -> 1,028. The in-memory runs had nothing else competing for the cores. In the node, every
prewarm thread spun for the whole block, taking CPU time from the sparse trie and proof workers
that compute the state root at the same time; the persistence wait rose from 0.06 to 0.78 ms.
Idle workers now wait on a condition variable. Second run: mean 31.9 -> 32.3 ms (+1.0%, within
the ±1.0% interval), Mgas/s +0.7%, end-to-end Mgas/s -3.3% (the persistence wait still rose).

So the regression is gone, but the execution phase that is 1.8x faster in memory does not shorten
`newPayload` on mainnet. Either the engine thread is not the critical path there (the state root
pipeline, or fixed costs), or speculation in the node is slow enough that the committer waits for
it (its state reads are cold cache and MDBX reads, unlike in memory). derek with `--debug.skip-state-root` on both sides separates the two:

| 500 blocks, 3 run pairs | main | branch | Change |
|---|---|---|---|
| Mean | 25.98 ms | 24.94 ms | -4.0% |
| P50 | 18.18 ms | 16.52 ms | -9.2% |
| Mgas/s | 1,508 | 1,660 | +10.1% |

So on mainnet the parallel execution does shorten the execution phase, and the state root then
absorbs the gain: main itself spends about 6 ms of its 32 ms mean on the root beyond execution,
and the branch finishes execution earlier only to wait longer for it. (A third arm, main with
only reuse enabled, failed twice while building on derek's runner.) The state root is now the
critical path, and it is the next phase.

## Phase 2: the state root as the critical path

How the root is computed today (`state_root_strategy/sparse_trie.rs`, `trie/sparse`):

- Each transaction's `StateUpdate` goes to one "trie-hashing" thread, which hashes addresses and
  slots serially, then to the "sparse-trie" thread, which applies leaves in batches. A leaf under
  a blinded node waits for a proof from the proof worker pools.
- Prewarm hints only prefetch proofs. They carry no values, so no leaf is applied before block
  execution commits its transaction.
- After the last transaction, the sparse-trie thread drains outstanding proof rounds (several
  round trips after removals that collapse onto blinded siblings), then serially returns the
  storage tries, hashes the upper account trie and collects trie updates. Only subtrie hashing
  and large storage passes use rayon.
- While updates keep arriving, `make_progress` applies no leaves after the first batch of 64
  messages: it applies buffered updates, computes storage roots and promotes account updates only
  when the update queue is empty and no proof results are pending. A producer that streams an
  update per transaction without gaps therefore pushes nearly all trie work past the last
  transaction. The faster the committer, the more continuous the stream, which is why Phase 1's
  execution gain went into the root. In tempo's bench-e2e the builder's state-root wait is a fixed
  ~28-30 ms p50 regardless of block size, and prefetch hints cut only ~3 ms of it
  (tempoxyz/tempo#8979), so the deferral, not proof fetching, dominates.
- No metric measures the time from the last update to the root; `state_root_duration` starts
  after post-execution validation.

Speculation already knows about 99% of the block's final writes within the first few
milliseconds, long before block execution commits them. The plan:

1. Measure the tail on derek: a histogram from `FinishedStateUpdates` to the root, and the number
   of proof rounds after it.
2. Bounded trie work while updates arrive: apply buffered updates, storage roots and account
   promotions every N updates or after a time slice even when more are queued, as zempo's
   `root_ahead` applies a window of changes beside execution, so that the work after the last
   transaction is only the last window. Leaving gaps in the stream from the producer side does
   not do it: tempo's builder sending updates in bursts of 32 (tempoxyz/tempo#8984) moved its
   finish p50 only from 27.6 to 26.9 ms, so the task itself has to schedule the work.
3. Root ahead of commit: feed the sparse trie the predicted values of stored speculative results,
   so that proofs are fetched and storage tries hashed while block execution is still running.
   When block execution commits a transaction, only leaves whose committed value differs from the
   applied prediction are rewritten. The sparse trie already tolerates rewriting a leaf many
   times per block, so this changes when leaves arrive, not what the root is.
4. Parallel finish: hash addresses on the workers that produced the results instead of one
   thread, and collect storage trie updates in parallel.

The other critical-path items, once the root no longer hides execution:

| Component | Evidence | Plan |
|---|---|---|
| Commit loop | 12% read-set validation, 17% `PredictedState::update`, 4% `BlockState::commit_account_changes` on the committer, in memory | Cheaper prediction storage (an arena per block instead of a `DashMap` of sorted vectors). Validating against predictions instead of the state was tried and is unsound: predictions omit some lifecycle changes, and an existing test caught it |
| Dependency chains | 27 stale results per block, 2.4 ms of serial re-execution in memory | Merge the effects of common conflicts the way balances are rebased; a faster interpreter or JIT for chain links |
| Execution cache inserts | Tempo profile: `ExecutionCache::insert_state` 5.2% of the engine thread | Move inserts off the engine thread, back the tables with huge pages |
| Proof worker pools | A tempo node logs "Spawning proof worker pools" with 32 storage and 32 account workers for every block on 16 threads | Start the pools once and size them to the cores |
| Keccak cache | Tempo profile: sparse-trie hashing thrashes the global keccak cache, so the EVM's mapping-slot hashes miss | Bypass the global cache for trie hashing |

## Safety

The new path is behind `--engine.prewarm-reuse` (default on in the branch, `=false` restores
sequential execution).

- `reuse::tests::fuzz_reuse_matches_sequential_execution` generates blocks of transactions that
  depend on each other through balances, nonces, storage, the beneficiary and destroyed accounts,
  speculates them in a random order with and without beneficiary predictions, and asserts the
  receipts, bundle state and streamed state updates of sequential execution. 512 cases run in CI;
  a local run of 50,000 passed. It fails if result validation is skipped.
- `corpus_reuse` and `corpus_parallel` replay recorded mainnet blocks and assert identical receipts
  and bundle state for every mode. Corpus files are written by `exec-replay --dump` and are not
  checked in (3.6 GB for 500 blocks).
- `e2e::prewarm_reuse` has a follower validate blocks of dependent transactions with reuse. It
  fails if result validation is skipped.
- `prewarm::tests::speculation_queue_claims_arrived_transactions_in_order` covers the queue.

Canary to add before enabling by default: a shadow mode that executes a sampled block
sequentially off the critical path and compares receipts and bundle state, recording a metric
and an error log on mismatch.

## Measuring

```bash
# record and replay blocks from a read-only archive datadir
example-exec-replay --datadir <datadir> --from <block> --count 500 --runs 3 --dump corpus.jsonl
# compare execution modes on the corpus
RETH_BLOCK_CORPUS=corpus.jsonl RETH_PARALLEL_WORKERS=8 \
  cargo test --profile profiling -p reth-engine-tree --lib corpus_parallel -- --ignored --nocapture
```
