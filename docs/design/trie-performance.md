# Sparse trie calculation and persistence

These changes optimize Reth's existing Ethereum Merkle Patricia Trie. They use the same
Keccak/RLP node encoding, compact branch records, and legacy/packed MDBX tables. They
require no database migration or configuration change. Ethereum's node and child-reference
encoding is described in the [Merkle Patricia Trie specification overview](https://ethereum.org/en/developers/docs/data-structures-and-encoding/patricia-merkle-trie/).

## Implementation

- Hash dirty subtries through mutable references instead of extracting them, sorting their
  paths, and restoring them. A traversal of dirty upper branches also handles deletions
  that leave dirty branches but no dirty leaves. Cached roots return immediately.
- Derive persistence masks while gathering child RLP nodes, and reuse those RLP nodes to
  collect persisted hashes. This avoids extra arena walks during branch hashing.
- Resume depth-first traversal at the next child nibble using a masked iterator, avoiding
  repeated scans of children that have already been visited.
- Skip identical persisted branches. For sorted storage updates, advance the database
  cursor after a matched or inserted row instead of seeking again. If the next row falls
  short of the target, use seeks for the remaining batch, bounding the speculative-read overhead for sparse
  updates. Deletions reset cursor advancement; exhausted ranges avoid further reads.
- Skip scanning retained storage tries for account-only proofs, and stop the scan after
  finding the last storage proof target.

Candidate `a0d8b420cd`, retained at merge `2042e313a2`, adds two changes to these trie improvements.
It sorts sparse-trie leaf updates by their fixed `B256` keys instead of comparing unpacked
64-nibble paths. These orders are equivalent for the unique, fixed-length input keys, so
update grouping and proof callback order remain unchanged. Each MDBX cursor write also
borrows its cached operation metrics instead of cloning and dropping their `Arc`; metrics
counts and duration boundaries remain enabled and unchanged. This applies to all MDBX
cursor write operations.

The candidate retains the original FIFO reclamation, mapped writes, prefault behavior,
and durable synchronization settings. The rejected LIFO, prefault-off, and buffered-write
experiments are not included.

The storage writer keeps the existing delete/upsert operations and transaction boundaries.
The returned entry count still counts processed nonempty paths, including unchanged nodes
and missing deletions.

## Mainnet replay with Derek

All comparisons use baseline `eadb887f1c1698e7bd932f60a0c654dccccd081f`, 500 measured mainnet
blocks after 125 warmup blocks, and six independent runs per variant in Derek's ABBA order.
The snapshot is restored for each process. State-root verification remains enabled, and
node logs confirm the sparse-trie strategy. The benchmark's bootstrap, confidence levels,
and practical significance thresholds are unchanged.

The first two candidates were neutral in Derek's configured metrics:

| Candidate | Run | Mean payload latency | Persistence wait |
| --- | --- | ---: | ---: |
| Trie hashing and sorted writes (`d14a096bbf`) | [33862537836](https://github.com/paradigmxyz/reth/actions/runs/33862537836) | +0.035% ±0.459% | −2.84% ±1.22% |
| Also stop proof-cache scans early (`33c491663d`) | [33864115641](https://github.com/paradigmxyz/reth/actions/runs/33864115641) | +0.16% ±0.84% | −2.14% ±1.20% |

These results do not establish an overall payload-latency improvement. The second run's
final root phase decreased by 7.08% ±2.37%, and trie-write service time per completed
persisted block decreased by 1.76% ±0.70%. Persistence crosses measurement boundaries:
the second comparison completed 3,016 baseline versus 3,024 candidate persisted blocks.
The log and metric-counter audits agree for every process; the per-block normalization
does not eliminate differences in the particular boundary blocks.

Derek's reported wall-clock metric sums server newPayload and client forkchoiceUpdated
latencies over all measured rows; it is not elapsed replay time. Persistence wait measures
engine backpressure, and the final root phase measures remaining work after overlap with
execution. These quantities must not be described as pure database writes or total hashing
CPU time.

The runners use AMD EPYC 4585PX CPUs with 16 physical cores online, SMT disabled, and the
node pinned to CPUs 1–15. Builds use rustc 1.98.1, the `profiling` profile, and
`RUSTFLAGS='-C target-cpu=native'`, with ASM Keccak and global Keccak caching enabled by
the node's default features. The native/cache kernel verification below matches those
compiler and hash settings. The earlier kernel matrix used rustc 1.96.1 without explicitly
enabling the global hash cache.

The buffered-write candidate `8c1f054c7e` was rejected and reverted. In
[run 33866784894](https://github.com/paradigmxyz/reth/actions/runs/33866784894), payload
latency was neutral, while persistence wait regressed by 32.55% ±2.62%. Trie-write service
time per completed persisted block increased by 28.77% ±3.54%, and MDBX commit time
increased by 49.42% ±2.00%. Recorded explicit writes added 8.762 ms per canonical block;
synchronization saved 1.379 ms, and garbage collection added only 0.0079 ms. Mean sampled
peak RSS increased by 164 MiB, and sampled peak jemalloc allocation increased by 259 MiB.
These are sampled peaks, not true process or system memory maxima. The local write-mode
benchmark below did not predict this full-node regression.

The intermediate prefault-off candidate `6f1a3a2fe0` was rejected. In
[run 33866279619](https://github.com/paradigmxyz/reth/actions/runs/33866279619), persistence
wait improved by 34.00% ±0.99%, while mean payload latency regressed by 2.48% ±0.65% and
p99 regressed by 49.73% ±7.56%. Selecting only its persistence result would hide a material
latency regression. The local cold test below independently ruled out that default before
the mainnet result arrived.

The LIFO-reclaim candidate `4355f17739` was rejected in
[run 33869169639](https://github.com/paradigmxyz/reth/actions/runs/33869169639). Mean payload
latency and reported wall clock remained neutral, while persistence wait regressed by
9.25% ±1.33%. MDBX save-blocks work increased by 8.76% ±0.87%, with commit nearly flat.
The state-write phase rose from 0.601 to 2.153 ms per canonical block, and hashed-state
work from 8.406 to 9.565 ms. These explain essentially all the added time, after a small
trie-persistence saving. Commit-GC wall time increased only 0.0036 ms per block.

Sampled peak RSS was 676 MiB lower, while sampled peak jemalloc allocation changed by less
than 0.2%; the lower resident footprint did not yield faster persistence. Eleven of twelve
append-log/counter checks agree exactly; a seven-block append within the first scrape's
millisecond explains the remaining ordering ambiguity, which is preserved in the audit.
The archived metrics do not expose MDBX probe counts or persistence-thread page faults,
so they cannot establish a particular residency-probe mechanism. The
[full LIFO assessment](trie-performance-data/derek/iteration-5/assessment.md) records the
phase, completed-work, timestamp, and memory limitations.

The combined leaf-sort and cursor-metrics candidate `a0d8b420cd` remained neutral in
[run 33870980404](https://github.com/paradigmxyz/reth/actions/runs/33870980404), using the
same unprofiled 500/125/six-pair configuration and original FIFO/mapped-write/prefault settings.
Mean payload latency changed by +0.094% ±0.456%, reported wall clock by +0.064% ±0.468%,
and p99 by +0.060% ±1.209%. Persistence wait decreased 3.05% ±1.07%, below its 5% practical
floor. These component changes are retained, but the result does not establish the
requested overall performance improvement.

The diagnostic final-root phase decreased 9.37% ±3.24%, from 337.1 to 305.5 µs per
canonical block. Processing updates decreased from 8.699 to 8.405 ms, while channel wait
increased from 4.986 to 5.176 ms and revealing proofs from 4.446 to 4.502 ms. Total root-task
elapsed time remained flat at 23.04 versus 23.02 ms. The observed phase changes do not
establish CPU savings or a causal explanation for how work overlapped.

Trie-persistence service time per completed persisted block decreased 2.53% ±0.80%, from
19.214 to 18.729 ms. All twelve append-log/counter checks agree, but the windows include
3,024 baseline versus 3,008 candidate persisted blocks; normalization cannot remove the
different boundary work. MDBX save time decreased 2.17% and commit time 1.26%, both neutral.
Sampled peak RSS was 30 MiB lower and peak jemalloc allocation 3 MiB lower; these are
descriptive samples, with the same memory-accounting limitations as earlier comparisons.

The official `hash-post-state` worker target improved 12.93% ±4.83%, saving about 41 µs
per canonical block. In the sparse strategy this worker normally waits for the event loop
to send already hashed state. Its elapsed duration includes that receive wait; it does not
execute the changed leaf sort or directly measure hashing CPU. Exported cache and task
counters also cannot reconstruct individual sort batch sizes. The
[sixth-run assessment](trie-performance-data/derek/iteration-6/assessment.md) preserves the
official verdict, phase details, completed-work checks, and attribution limits.

## Choosing the database write mode

A diagnostic CPU profile identified residency probes as approximately 42% of trie-write
CPU time. Disabling MDBX's optional prefault writes removes that work, but can introduce
reads of obsolete destination pages when those pages are cold. The alternative uses MDBX's
transaction-owned page buffers and writes them during commit.

The local comparison uses rustc 1.98.1 with native CPU tuning, three independent processes,
and 12 measured samples per mode and case. Large cases seed 98,304 account/storage branch
records and replace 12,288 per transaction after four rounds establishing reclaimed-page
reuse. Every sample commits durably, then closes the database, reopens it, and checks all
logical records. Reopen and verification are outside the timed write transaction.

| Database case | Mapped writes, prefault on (ms) | Mapped writes, prefault off (ms) | Transaction buffers (ms) |
| --- | ---: | ---: | ---: |
| Initial inserts | 1.115 | 1.018 | 1.062 |
| Cached pages | 57.152 | 57.180 | 55.679 |
| Cold pages | 172.585 | 1,155.446 | 170.679 |

Values are pooled medians of 36 samples, not confidence intervals. Buffered writes improve
the cached case by 2.4–2.9% in each process; cold results vary in both directions and are
neutral. The small initial-insertion case is noisy. All 324 measured samples pass snapshot
equality. See the [complete results](trie-performance-data/write-mode/summary.json) and
[build and fixture metadata](trie-performance-data/write-mode/metadata.json).

The cold fixture closes its mappings and applies `POSIX_FADV_DONTNEED` only to its own
disposable database file. Residency checks find zero resident pages in 107 of 108 cold
samples; one sample has 4 of 18,432 pages resident. This is a file-cold test, not a simulation
of whole-node memory pressure. Disabling prefault writes roughly doubles major faults and
is 6.69× slower in this case, so that default is rejected. Transaction buffers avoid that
extra fault count. The Derek comparison rejected buffered writes despite the favorable local result.
These small-database measurements alone do not establish a production win.

A separate experiment disabled the supported `MDBX_USE_MINCORE` build option while
keeping prefault writes enabled. Four processes per variant, in two ABBA rounds, showed
warm durable writes 5.35% slower with reopened mappings and 5.01% slower with persistent
mappings; cold writes were neutral. Removing residency checks caused redundant prefault
writes on warm pages. All 288 measured transactions passed snapshot checks, with 24 final
abort/reopen checks. This configuration was rejected before Derek; no vendored source was
modified. [Diagnostic method and results](trie-performance-data/mincore/report.md)
include the build option, fixture limits, raw samples, and reproduction commands.

## Components in the sixth replay

Two component comparisons use retained `33c491663d` as their baseline. Their local results
motivated the combined `a0d8b420cd` replay. The components are retained, while Derek 6's
overall metrics remain neutral.

The packed-key comparator (`f4fb30dfaf`) uses the same Rust 1.98.1/native/ASM/global-cache
root harness as the next section: one or fifteen workers, two ABBA rounds, four processes
per variant, and 100 timed samples per case. These rows retain branch updates and time
applying updates, calculating the root, and extracting updates, without database I/O.
Positive reductions mean faster.

| Keys | Changed | One worker (µs) | Reduction | Fifteen workers (µs) | Reduction |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1,000 | 100 | 109.70 → 104.44 | 4.80% | 100.85 → 96.55 | 4.27% |
| 1,000 | 1,000 | 437.82 → 363.29 | 17.02% | 372.87 → 287.42 | 22.92% |
| 32,768 | 1 | 7.08 → 7.15 | −1.05% | 8.71 → 9.08 | −4.32% |
| 32,768 | 3,276 | 3,990.96 → 3,690.44 | 7.53% | 1,297.90 → 999.30 | 23.01% |
| 32,768 | 32,768 | 17,812.63 → 14,220.84 | 20.16% | 9,418.59 → 5,553.39 | 41.04% |

The original fifteen-worker matrix also had a 13.25% slower single-change retained-root
control (6.958 → 7.880 µs) and a 40 ns higher cached-root median (65 → 105 ns). The comparator
does not execute in root-only timing, and a one-element sort performs no comparisons.
Those observations remain in the original data. Separate focused repeats used the same
binaries, four ABBA rounds and 200 samples, and did not reproduce the large control gaps:

| Focused fifteen-worker control | Baseline | Candidate | Candidate change |
| --- | ---: | ---: | ---: |
| One-change retained root, 32,768 keys | 7.9950 µs | 8.0205 µs | +0.32% |
| One-change retained update + root | 9.7085 µs | 9.5975 µs | −1.14% |
| Cached retained root, 32,768 keys | 85.5 ns | 90.0 ns | +4.5 ns |

All focused-control process quartiles overlap. They establish neither a comparator
regression nor exact zero overhead. Excluded setup, randomized map layout, cache state,
and Rayon scheduling can still affect this repeated-fixture benchmark; no particular
cause of the original anomaly is established. The [component report](trie-performance-data/packed-sort/report.md),
[original one-worker](trie-performance-data/packed-sort/threads-1/runs.csv) and
[fifteen-worker](trie-performance-data/packed-sort/threads-15/runs.csv) observations, and
[focused single-change](trie-performance-data/packed-sort/focused-single-15/runs.csv) and
[cached-root](trie-performance-data/packed-sort/focused-cached-15/runs.csv) repeats remain
separate. No focused values replace or get pooled into the original matrix.

The cursor-metrics component (`0798ad069c`) uses Rust 1.98.1/native profiling, CPU 0, two
ABBA rounds, and a warm persistent database. Every sample replaces 12,288 of 98,304 packed
account/storage branch records and commits durably. Each process alternates metrics-on
and metrics-off controls, with 40 samples per mode after four warmups: 640 measured rows.
Real atomic counters record exactly 20,480 operations per metrics-on sample; disabled
controls record zero. The fixture's values do not cross the histogram timing threshold;
separate regression tests cover large-value timing and errors.

Percentages below are candidate/baseline changes, so negative means faster. The control
normalization divides the metrics-on ratio by the corresponding metrics-off ratio; it is
a descriptive comparison, not a confidence interval.

| Baseline/candidate processes | Write, metrics on | Write, metrics off | Write normalized | Durable median, metrics on | Durable mean, metrics on |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 / 2 | −1.03% | −0.11% | −0.92% | +1.19% | +1.39% |
| 4 / 3 | −1.96% | −1.55% | −0.42% | +0.33% | +5.79% |
| 5 / 6 | −2.23% | −0.61% | −1.62% | −0.38% | −0.38% |
| 8 / 7 | −1.24% | −0.20% | −1.04% | −0.10% | −0.42% |

Write-phase medians improve, including the normalized controls, but total durable
persistence has no demonstrated improvement. Commit medians shift from roughly 44–45 ms
early in the capture to 27 ms later; baseline process 4 straddles the change. Its paired
durable mean increase is 5.79% with metrics on and 6.60% with metrics off. Pooled durable
medians are sensitive to that mixture and must not replace the paired comparisons.
Untimed full snapshots warm live pages after every commit; close/reopen checks validate
logical persistence, not crash recovery. See the [metrics component report](trie-performance-data/metrics-borrow/report.md),
[per-process and paired statistics](trie-performance-data/metrics-borrow/summary.json),
and [build/fixture metadata](trie-performance-data/metrics-borrow/metadata.json).

## Native compiler and global-cache verification

A separate matrix compares the original baseline with retained candidate `33c491663d`.
Both use rustc 1.98.1, the `profiling` profile, `target-cpu=native`,
`alloy-primitives/asm-keccak`, and `alloy-primitives/keccak-cache-global`. One Rayon worker
runs on CPU 1; fifteen workers run on CPUs 1–15. Two ABBA rounds give four processes per
variant and worker count, each with 100 measured samples after five warmups. This is 56
case comparisons using the same 28-case harness as the initial matrix.

The retained candidate has lower medians in 54 of 56 cases, including every dirty-root
case. The table shows 32,768-leaf tries retaining branch updates. Times are medians of
process medians, and reductions are descriptive, without confidence intervals.

| Operation | Workers | Changed | Baseline (ms) | Retained (ms) | Reduction |
| --- | ---: | ---: | ---: | ---: | ---: |
| Root | 1 | 3,276 | 3.1809 | 3.0039 | 5.56% |
| Root | 1 | 32,768 | 9.5369 | 9.1300 | 4.27% |
| Update + root + take | 1 | 3,276 | 4.1080 | 3.9904 | 2.86% |
| Update + root + take | 1 | 32,768 | 18.1727 | 17.9161 | 1.41% |
| Root | 15 | 3,276 | 0.5957 | 0.5237 | 12.08% |
| Root | 15 | 32,768 | 1.2346 | 1.1154 | 9.65% |
| Update + root + take | 15 | 3,276 | 1.3509 | 1.3084 | 3.15% |
| Update + root + take | 15 | 32,768 | 9.4365 | 9.4978 | −0.65% |

The full-update, fifteen-worker case is 0.65% slower: baseline process quartiles are
9.3945–9.5110 ms, and candidate quartiles are 9.4620–9.5414 ms. The other slower median is
the single-worker, one-changed-leaf, discard-updates case at 32,768 leaves: 6.6275 versus
6.6325 µs, or 0.08% slower, also with overlapping quartiles. Neither observation is omitted.
Cached-root medians decrease by 84–93%, but those very short calls include timer overhead
and do not measure fresh hashing.

The hash features match the node, but the fixture repeatedly uses identical values.
Fixture construction, reference hashing, and warmup populate the process-global hash cache;
cache hit rates are not measured. The harness uses the system allocator rather than node
jemalloc, and excludes cloning, input allocation, reference checks, and database I/O.
These are repeated-fixture, cache-warm kernel results. Their absolute times are not directly
comparable to the earlier matrix, which lacked explicit global caching and used a different
compiler and worker count. They do not change the retained candidate's neutral Derek verdict.

See the [complete native/cache matrix](trie-performance-data/native-cache/summary.csv),
[assessment](trie-performance-data/native-cache/assessment.md),
[build provenance](trie-performance-data/native-cache/build-metadata.json), and raw
[one-worker](trie-performance-data/native-cache/threads-1/runs.csv) /
[fifteen-worker](trie-performance-data/native-cache/threads-15/runs.csv) process summaries.
All 16 processes check every root against Alloy HashBuilder and have identical case/root
sets. [Verification and counts](trie-performance-data/native-cache/verification.json)
are retained with the measurements.

## Initial trie kernel measurements

These earlier measurements use rustc 1.96.1, portable or assembly Keccak without explicitly
enabling `keccak-cache-global`, and one or four Rayon workers. They compare the original
baseline with the trie and cursor changes in `d14a096bbf`, before the subsequent proof-cache
and database-mode changes.

All 112 root benchmark combinations (portable/assembly Keccak, one/four workers, 28 cases)
have lower candidate medians. With assembly Keccak, four workers, and retained updates:

| Operation | Leaves | Changed | Baseline (µs) | Candidate (µs) | Reduction |
| --- | ---: | ---: | ---: | ---: | ---: |
| Root | 1,000 | 1 | 4.208 | 4.027 | 4.3% |
| Root | 1,000 | 100 | 66.816 | 60.584 | 9.3% |
| Root | 1,000 | 1,000 | 175.124 | 151.329 | 13.6% |
| Root | 32,768 | 1 | 6.518 | 6.327 | 2.9% |
| Root | 32,768 | 3,276 | 1,062.710 | 1,007.708 | 5.2% |
| Root | 32,768 | 32,768 | 3,738.291 | 3,611.253 | 3.4% |
| Update + root + take | 1,000 | 100 | 85.852 | 78.868 | 8.1% |
| Update + root + take | 1,000 | 1,000 | 313.959 | 287.405 | 8.5% |
| Update + root + take | 32,768 | 3,276 | 1,500.474 | 1,458.840 | 2.8% |
| Update + root + take | 32,768 | 32,768 | 7,827.167 | 7,696.566 | 1.7% |

See the [complete assembly-Keccak matrix](trie-performance-data/asm/summary.csv)
and [per-process measurements](trie-performance-data/asm/runs.csv).

The portable-Keccak matrix has lower candidate medians in all 56 cases (28 cases at each
worker count). The following rows use four workers and retain branch updates. Times are
microseconds; reductions compare the median elapsed times.

| Operation | Leaves | Changed | Baseline (µs) | Candidate (µs) | Reduction |
| --- | ---: | ---: | ---: | ---: | ---: |
| Root | 1,000 | 1 | 4.844 | 4.604 | 5.0% |
| Root | 1,000 | 100 | 74.736 | 66.730 | 10.7% |
| Root | 1,000 | 1,000 | 197.171 | 171.127 | 13.2% |
| Root | 32,768 | 1 | 7.359 | 7.048 | 4.2% |
| Root | 32,768 | 3,276 | 1,211.170 | 1,132.047 | 6.5% |
| Root | 32,768 | 32,768 | 4,395.732 | 4,189.164 | 4.7% |
| Update + root + take | 1,000 | 100 | 93.471 | 85.040 | 9.0% |
| Update + root + take | 1,000 | 1,000 | 335.781 | 309.130 | 7.9% |
| Update + root + take | 32,768 | 3,276 | 1,645.046 | 1,586.170 | 3.6% |
| Update + root + take | 32,768 | 32,768 | 8,483.006 | 8,306.966 | 2.1% |

Cached-root lookups with retained updates decrease from 0.3805 to 0.0400 µs (1,000 leaves)
and 0.8070 to 0.0705 µs (32,768 leaves). These are lookup improvements, not fresh hashing.
See the [complete portable matrix](trie-performance-data/portable/summary.csv)
and [per-process measurements](trie-performance-data/portable/runs.csv)
for both worker counts and retention modes.

Durable persistence uses three processes per case, each with 100 samples after three
warmups. These are medians of the three process medians, in milliseconds:

| Encoding | Updates | Baseline (ms) | Candidate (ms) | Reduction |
| --- | --- | ---: | ---: | ---: |
| Legacy | Unchanged | 3.261 | 0.528 | 83.8% |
| Packed | Unchanged | 2.999 | 0.526 | 82.5% |
| Legacy | Replace | 3.354 | 2.819 | 15.9% |
| Packed | Replace | 2.999 | 2.999 | 0.0% |
| Legacy | Resize | 5.511 | 4.964 | 9.9% |
| Packed | Resize | 5.091 | 4.653 | 8.6% |
| Legacy | Mixed | 2.931 | 2.817 | 3.9% |
| Packed | Mixed | 2.999 | 2.559 | 14.7% |
| Legacy | Sparse replace | 1.185 | 1.192 | -0.6% |
| Packed | Sparse replace | 1.121 | 1.127 | -0.5% |
| Legacy | Sparse resize | 2.415 | 2.427 | -0.5% |
| Packed | Sparse resize | 2.175 | 2.176 | -0.0% |
| Legacy | Append | 2.818 | 2.358 | 16.3% |
| Packed | Append | 2.443 | 1.999 | 18.2% |

Unchanged batches are 6.17× faster with legacy keys and 5.70× with packed keys. Dense
replacements, resizes, and appends improve, except packed replacement is effectively
unchanged. Sparse batches have small aggregate regressions, within 0.7%, and overlapping
quartiles. Durable I/O varies between runs: one legacy sparse-resize run had a 19.6%
median regression despite nearly identical wide quartile ranges. All individual results
are retained in the [persistence report](trie-performance-data/persistence/report.md)
and [per-repeat comparisons](trie-performance-data/persistence/comparison.csv).

## Initial benchmark method

The baseline is commit `eadb887f1c1698e7bd932f60a0c654dccccd081f`, with only the benchmark
harness and its Cargo registration added. Baseline and candidate use Rust 1.96.1 and the
workspace `profiling` profile (optimized, thin LTO). Measurements were made on an AMD EPYC
4585PX with boost disabled. Root runs use CPU affinity 0–3 and explicitly select one or
four Rayon workers. Persistence runs use CPU 0. Compilation and fuzzing are paused during
measurements.

`crates/trie/sparse/benches/state_root.rs` uses deterministic Keccak-distributed keys and
RLP-encoded storage values, at 1,000 and 32,768 leaves. Cases cover cached roots and changes
to one leaf, 10% of leaves, or every leaf, with and without retained branch updates.
`root_dirty` times hashing applied changes. `update_root` times applying changes, hashing,
and taking branch updates. Fixture construction, cloning, input allocation, and database
I/O are excluded. Every sample checks its root against Alloy's independent streaming
`HashBuilder`.

Both portable Keccak and `alloy-primitives/asm-keccak` are measured; the latter is enabled
by the Reth binary's default features. Each variant uses two ABBA rounds, giving four
processes per version and worker count, each with 100 measured samples after five warmups.
Reported values are medians of process medians. The CSVs retain per-process quartiles;
they are descriptive statistics, not confidence intervals.

`crates/trie/db/benches/storage_persistence.rs` compares the baseline writer with the
production writer in the same binary. Each sample starts from a fresh database containing
4,096 branch records. The timed region includes opening a write transaction, applying
updates, and committing with `SyncMode::Durable`. Initialization and verification are
outside the timer. Baseline/candidate order alternates, and each sample closes and reopens
the database to check decoded record equality. Cases include identical values, replacements,
resizing, mixed changes/deletions, sparse changes, and appends, in both key encodings.

These are synthetic trie kernel and durable branch-persistence measurements. They do not
measure full mainnet block execution, proof fetching, state hashing, cold-cache database
reads, or whole-node throughput. The persistence fixture uses synthetic compact branch
records; separate integration tests validate actual state and storage roots after reopen.

## Reproduction

The comparison runner requires Linux, `taskset`, and Python 3.11 or newer. Build each root
benchmark from the desired revision with the same harness and compiler. In a baseline
checkout, copy `crates/trie/sparse/benches/state_root.rs` and its `[[bench]]` registration
from `crates/trie/sparse/Cargo.toml` before building. Copy the executable shown on Cargo's
`Executable benches/state_root.rs (...)` line to a distinct path before rebuilding.
Use separate target directories (`CARGO_TARGET_DIR`) for separate worktrees to avoid
Cargo reusing artifacts with relative source paths.

```sh
cargo bench --locked --profile profiling -p reth-trie-sparse --bench state_root --no-run
cargo bench --locked --profile profiling -p reth-trie-sparse --bench state_root \
    --features alloy-primitives/asm-keccak --no-run

python3 scripts/bench-trie.py /path/to/baseline /path/to/candidate \
    --output bench-work/trie-comparison --threads 1,4 --cpus 0-3 --samples 100 --rounds 2

cargo bench --locked --profile profiling -p reth-trie-db --bench storage_persistence --no-run
MPT_BENCH_SAMPLES=100 taskset -c 0 /path/to/storage_persistence
```

The runner records executable checksums, machine metadata, raw per-process CSVs, and a
summary. Run the persistence executable three times and retain all outputs.

To reproduce the native/cache matrix, build both `eadb887f1c` and `33c491663d` with the same
harness in separate worktrees and target directories, preserving each executable:

```sh
CARGO_TARGET_DIR=/tmp/reth-native-baseline-target RUSTFLAGS='-C target-cpu=native' \
    cargo +1.98.1 bench --locked --profile profiling -p reth-trie-sparse \
    --features alloy-primitives/asm-keccak,alloy-primitives/keccak-cache-global \
    --bench state_root --no-run

python3 scripts/bench-trie.py /path/to/native-baseline /path/to/native-retained \
    --output bench-work/native-cache-1t --threads 1 --cpus 1 --samples 100 --rounds 2
python3 scripts/bench-trie.py /path/to/native-baseline /path/to/native-retained \
    --output bench-work/native-cache-15t --threads 15 --cpus 1-15 --samples 100 --rounds 2
```

Use a different `CARGO_TARGET_DIR` for the retained candidate. Compilation, other benchmarks,
and large artifact copies must finish before starting either timing command.

The write-mode experiment uses the
[diagnostic harness patch](trie-performance-data/write-mode/diagnostic-harness.patch) on
commit `6f1a3a2fe047df40b374a9881fd51360df6963d2` in a separate checkout. The patch adds
temporary mode selection for comparing all three backends in one executable; those
configuration overrides are not part of the production candidate. After applying it:

```sh
CARGO_TARGET_DIR=/tmp/reth-write-mode-target RUSTFLAGS='-C target-cpu=native' \
    cargo +1.98.1 bench --locked --profile profiling -p reth-trie-db \
    --bench prefault_write --no-run
MPT_PREFAULT_SAMPLES=12 taskset -c 0 /path/to/prefault_write > run-1.csv
```

Repeat in three independent processes. The harness requires Linux for its file-local
cache eviction and residency checks. It never drops global caches.

The mainnet comparison uses the repository's unchanged Derek workflow:

```sh
gh workflow run bench.yml --repo paradigmxyz/reth --ref perf/trie-root-persistence \
    -f baseline=eadb887f1c1698e7bd932f60a0c654dccccd081f \
    -f feature=33c491663d96c545e49e8af4cb6edc233132f0ce \
    -f blocks=500 -f warmup=125 -f run_pairs=6 -f cores=0 -f mode=engine \
    -f otlp=false -f metrics=true -f samply=false -f tracing_chrome=false -f slack=never
```

## Correctness

The latest full-workspace validation ran on `a0d8b420cd`, which has identical source to
retained main merge `2042e313a2`. All checks passed without production changes.

- Full workspace nextest on rustc 1.98.1: 3,819 passed; 38 skipped. One RPC test passed
  on its second attempt after HTTP port 8545 was occupied on its first attempt.
- Whole-workspace nightly Clippy passed with all features, libraries, examples, tests,
  and benches, with warnings treated as errors. Private workspace documentation built
  all 141 outputs without warnings. Nightly formatting, Zepter, tracked-TOML formatting,
  and clean-diff checks passed. [Validation commands](trie-performance-data/validation.txt)
  and [structured results](trie-performance-data/validation-status.json) are retained
  alongside the benchmarks. The exact candidate also passed 236 focused database/trie tests.
- The sparse crate's 180 tests include 1,000 randomized mutation/pruning cases and every
  combination of 65,536 branch occupancy masks and 17 cursor restart positions.
- A deterministic differential run (`--cases 10000 --seed 1 --mask-sweep`) exercised
  4,448,976 operations, 641,072 root checks, 255,536 compact-node checks, 100,022 proof
  rounds, 90,000 in-memory sparse-cache rebuilds, and 40,000 prunes. Roots are checked against
  `triehash` and Alloy; compact records are derived independently from decoded RLP proofs.
- Persistence tests cover 128 generated operation streams, dense and sparse updates,
  resizing, duplicate paths, deletion, wiping, transaction abort/drop, and durable reopen.
- `sparse_root_persistence` writes real sparse storage and account trie updates, closes
  MDBX, and verifies storage and state roots with the database trie walker and `triehash`,
  using both legacy and packed encodings.
- The rejected buffered-write experiment also passed mode, durable synchronization,
  commit/reopen, and aborted-transaction checks; functional correctness did not establish speed.

```sh
cargo nextest run --locked -p reth-trie-sparse
cargo nextest run --locked -p reth-trie-db --test storage_persistence --test sparse_root_persistence
cargo run --locked --profile profiling -p reth-trie --features test-utils \
    --example sparse_differential_fuzz -- --cases 10000 --seed 1 --mask-sweep
```
