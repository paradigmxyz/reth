# Complete state trie database: corrected big-block benchmarks

Measured on dev-brian on **2026-09-25**, after restoring BundleState-based
execution reads and permitting normal sparse-trie reuse to skip the state-trie
overlay. Both profiling binaries come from commit
`d981f702d1af45d7e5d229a022011b333129d23b`; only the `state-trie-db` feature differs.

## Results

The execution slowdown is gone. Median server payload validation is
**125.42 ms old versus 124.08 ms new**; mean validation is 135.53 versus
133.76 ms (−1.3%). Mean BAL execution is **121.29 versus 119.65 ms (−1.3%)**.
Every run recorded **zero state-trie overlay computations**. The prior
September 24 result of 415.42 ms median validation in new mode measured the
implementation that routed execution through the trie overlay.

Proof work is lower with the new tables: mean account proof-job duration drops
29.5%, and mean storage proof-job duration drops 32.1%. Remaining state-root
wait averages 0.548 ms old versus 0.309 ms new (−43.6%). This is the wait after
execution, not the total time spent computing the root.

Persistence remains slower. Across all 600 blocks including shutdown, summed
`save_blocks` time increases **13.1%**. Sustained replay throughput, including
ordinary engine backpressure, is **0.949 Ggas/s old versus 0.801 Ggas/s new
(−15.6%)**. Nearly equal server validation time therefore does not imply equal
sustained ingestion throughput.

Durations below are **milliseconds**. Block-level rows have 1,650 samples per
mode (550 scored blocks × 3 runs); proof-job rows pool individual jobs.

| Metric | Old p50 | New p50 | Old p90 | New p90 | Old p99 | New p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Server payload validation | 125.420 | 124.082 | 149.593 | 149.706 | 406.823 | 389.907 |
| Client newPayload RPC | 150.000 | 149.000 | 180.000 | 178.000 | 45,683.050 | 55,228.180 |
| BAL execution | 112.684 | 110.895 | 132.967 | 132.377 | 367.789 | 357.032 |
| State-root wait | 0.0036 | 0.0022 | 0.0047 | 0.0039 | 13.2755 | 7.2388 |
| Account proof job | 0.888 | 0.550 | 2.903 | 2.083 | 11.235 | 9.284 |
| Storage proof job | 0.218 | 0.139 | 1.419 | 0.857 | 5.888 | 4.741 |
| Account proof service sum/block | 2,472.896 | 1,737.707 | 4,036.595 | 2,954.364 | 7,812.566 | 5,714.292 |
| Storage proof service sum/block | 1,023.582 | 691.943 | 1,977.617 | 1,347.748 | 5,783.537 | 3,902.376 |

The old runs contain 3,146,300 account jobs and 3,443,191 storage jobs; the new
runs contain 3,145,015 and 3,438,837. Job counts are nearly unchanged. Mean
account service sum/block falls from 2,766.41 to 1,950.15 ms (−29.5%); mean
storage service sum/block falls from 1,269.82 to 860.76 ms (−32.2%). These sums
include overlapping worker time and are not elapsed block time or CPU time.

### Persistence

`save_blocks` durations are **seconds per batch**:

| Window | Mode | Batches | p50 | p90 | p99 | Mean |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| After warmup, completed during replay | old | 42 | 43.99 | 76.28 | 87.27 | 40.83 |
| After warmup, completed during replay | new | 42 | 54.71 | 83.05 | 102.70 | 48.67 |
| All batches, including shutdown | old | 54 | 43.99 | 81.70 | 95.16 | 41.66 |
| All batches, including shutdown | new | 54 | 54.71 | 83.45 | 101.68 | 47.13 |

Both modes make 42 batches in the primary window and 54 overall. Primary
batches average 33.12 appended blocks old versus 34.19 new. Summed primary
save duration divided by summed appended blocks is **1.233 s/block old versus
1.424 s/block new (+15.5%)**.

Across all 600 blocks, mean cumulative save time per run is **749.79 s old
versus 848.32 s new (+13.1%)**, or 1.250 versus 1.414 s per appended block.
Each mode appends exactly 1,800 blocks across its three runs and fully flushes
the state. These are cumulative handler durations; persistence overlaps
execution, so they must not be added to replay wall time.

Client newPayload RPC p99 is **45.68 s old versus 55.23 s new**. This includes
engine persistence backpressure, which the server validation timer excludes.

### Repetitions

Each row scores the same 550 blocks after warmup. Replay duration runs from
the 50th to the 600th completed submission, including forkchoice updates and
persistence backpressure, and excluding shutdown.

| Run | Validation p50 (ms) | p90 (ms) | p99 (ms) | Mean (ms) | Replay (s) | Ggas/s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| new-1 | 124.49 | 149.88 | 392.98 | 133.71 | 684.87 | 0.817 |
| old-1 | 125.10 | 148.17 | 402.65 | 135.45 | 591.11 | 0.947 |
| old-2 | 125.53 | 150.14 | 398.92 | 135.58 | 598.12 | 0.936 |
| new-2 | 123.39 | 148.39 | 390.24 | 133.66 | 699.83 | 0.800 |
| new-3 | 124.31 | 150.62 | 364.30 | 133.90 | 711.20 | 0.787 |
| old-3 | 125.54 | 149.05 | 416.38 | 135.58 | 580.06 | 0.965 |

Including warmup gives mean validation of 137.09 ms old versus 134.61 ms new,
and mean execution of 122.93 versus 120.64 ms.

## Correctness and recovery

All **3,600 payloads returned VALID**, all 600 state roots match across all six
runs, and the logs contain no errors. Every run used BAL execution and the
sparse-trie state-root strategy, with zero state-trie overlay computations,
zero root-task timeouts, and zero successful serial fallbacks. Each run shut
down cleanly with Finish **24,979,600** and no partial-state-trie lag. The final
state root was
`0x378e9efc76298c47ca401a2c9da04ce0bc38a08f5d1a69ff9b71b95084200012`.

Final `schelk recover` restored Finish and partial-state-trie to **24,979,000**
and verified the promoted root
`0xc60084882e9a560a2679b26dbec4a5c7a6f5ac27a02985776706b82e45d7e254`.
`/schelk` is mounted with approximately **2.1 TiB available**. No benchmark
state was promoted.

Preliminary replays exposed correctness issues previously hidden by the trie
overlay: proof traversal read a known, masked parent; leaf masking could
suppress an execution value when only its short-key metadata changed; and
subtrie deletion redundantly requested a proof for an already-cached leaf
while waiting for its blinded sibling. These were fixed before the measured
series. Regression tests cover these cases; the final trie/sparse-trie suite
passed 283 tests. Failed attempts and diagnostic runs are preserved and
explicitly excluded in `manifest.json`.

## Method

Three runs per mode, in the order new, old, old, new, new, old. Each run replays
600 identical big blocks with merged block access lists (BALs), excluding the
first 50 blocks from the primary statistics: 550 measured blocks per run and
1,650 per mode. Percentiles use linear interpolation between ordered samples.
The pooled samples repeat the same workload three times; they are not 1,650
distinct blocks.

The corpus is the first 600 records of
`/schelk/bench/txgen-big-blocks-mainnet-24979001-1000-1G.ndjson`, using the txgen
big-block format. Each block contains 1,000,014,208–1,058,504,590 gas (mean
1,017,547,908). Each run executes 9,083,552 transactions from 20,195 source
blocks, 24,979,001–24,999,195, merged into synthetic blocks
24,979,001–24,979,600. All records have nonempty merged BALs. The SHA-256 of
the selected raw records is
`5a729f23dad32e014413ab6cb9a3f4a13c3c93bccd5c979201d00e1a77a55728`.

Before every run, `schelk recover` and `schelk mount` restore the same promoted
snapshot at block 24,979,000. The migration's read-only `--check` verifies the
persistence frontiers and migrated root. Linux page caches are then cleared.
The baseline contains both schemas; each mode reads and updates its respective
schema. There are no concurrent builds or replays and no network peers.

Hardware: AMD EPYC 4585PX, 16 cores / 32 threads, 62 GiB RAM, CPU governor
`performance`, Linux 6.8.0-110-generic. Builds use Rust 1.98.1 and the
`profiling` profile:

```sh
cargo +stable build --profile profiling -p reth-bb
cargo +stable build --profile profiling -p reth-bb --features state-trie-db
```

Both nodes use persistence threshold 50, memory block buffer 5, state masking
30, and state-root task timeout `0s` (disables serial timeout fallback). The
replay uses `bench send-blocks --wait-for-persistence never`, sending a
forkchoice update for each block. Normal engine persistence backpressure still
applies. Metrics are scraped every 500 ms. The preserved `run.py` and per-run
command JSON files contain the full invocation.

Both binaries have identical debug timing instrumentation for proof jobs and
persistence. `reth-bb` has a 900-second graceful shutdown timeout so pending
big-block persistence can drain. No builds or analysis ran during measured
replay or shutdown flushes.

## Measurement definitions

- **Payload validation:** the engine's server `newPayload` processing timer,
  excluding earlier persistence and cache waits. Client RPC duration is shown
  separately and includes those waits and transport overhead.
- **Execution:** elapsed time around BAL parallel block execution, including
  provider reads and rebuilding the BAL.
- **State-root wait:** time spent in `state_root_job.finish` after execution and
  post-execution checks. Proof and root work can already have completed in
  parallel; this is not total state-root computation time.
- **Account proof job:** account multiproof service duration, including dispatch
  and waiting for storage proofs, excluding initial job queue time.
- **Storage proof job:** storage proof calculator service duration. The old and
  new implementations can dispatch different numbers of jobs, including
  root-only requests. Per-block sums and job counts accompany per-job statistics.
  Worker durations overlap each other and execution; their sum is not elapsed
  block time or CPU time.
- **`save_blocks`:** persistence handler duration including database writes,
  commit, and BAL flush. Primary statistics include batches wholly after warmup
  that complete by the last replay submission. Shutdown flushes are reported
  separately. Batch sizes vary, so per-batch durations are accompanied by
  durations normalized by the number of appended blocks. State-trie masking
  means this is not the number of state-trie blocks written in that batch.

## Artifacts

All artifacts are preserved outside the recovered volume at
`/home/ubuntu/state-trie-db-benches-20260925`. They include binary hashes and
machine/build details (`manifest.json`), corpus metadata (`corpus.json`), exact
commands, compressed node logs and bench reports, metrics, checkpoint checks,
the driver (`run.py`), and analysis (`analyze.py`). Per-run CSVs and pooled
`results.json` retain the measurements behind the tables. Empirical latency
distributions are saved as `latency-distributions.png` and
`latency-distributions.pdf`, with their plotting script in `plot.py`.

The September 24 artifacts remain at
`/home/ubuntu/state-trie-db-benches-20260924`; their timings precede the
execution-overlay corrections and are excluded from this comparison.

See [the PoC documentation](state-trie-db.md) for the schema, migration, and
disk-space measurements.
