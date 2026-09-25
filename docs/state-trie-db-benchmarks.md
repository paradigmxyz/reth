# Complete state trie database: big-block benchmarks

Measured on dev-brian on 2026-09-24. This compares the current complete-state-trie
PoC with the legacy hashed state and compact trie implementation. Both binaries
come from commit `ea414547c90c47e3c8491b080f65c24c2d72d88f`; only the
`state-trie-db` feature differs.

These measurements precede the correction that restores BundleState-based
execution overlay reads. The measured PoC routed execution through the trie
overlay, adding full-overlay merge work to the execution path. These results
do not measure the corrected implementation.

## Results

The current PoC is slower overall on this workload. Median server payload
validation rises from **125.22 ms to 415.42 ms (3.32×)**. Mean validation rises
from **135.58 ms to 418.07 ms**, while mean BAL execution rises from
**121.21 ms to 404.33 ms**. Time outside execution is almost unchanged
(14.37 ms versus 13.75 ms). The measurements locate the added elapsed time
inside execution; they do not isolate a particular provider operation or prove
that the schema itself causes it.

Proof work improves modestly: summed account proof service time per block is
16.8% lower on average, and storage proof service time is 4.2% lower. Remaining
state-root wait is also lower, but the longer execution gives parallel root
work more time to finish. It should not be read as an equivalent reduction in
total root-computation cost.

All durations in the following table are **milliseconds**. Each block-level
row has 1,650 measured samples per mode; proof-job rows pool individual jobs.

| Metric | Old p50 | New p50 | Old p90 | New p90 | Old p99 | New p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Server payload validation | 125.221 | 415.416 | 148.811 | 497.807 | 416.347 | 1,231.626 |
| Client newPayload RPC | 150.000 | 441.000 | 177.000 | 523.000 | 47,081.300 | 48,116.060 |
| BAL execution | 112.483 | 402.242 | 132.278 | 483.914 | 370.241 | 1,193.860 |
| State-root wait | 0.0034 | 0.0022 | 0.0046 | 0.0033 | 18.6615 | 3.4807 |
| Account proof job | 0.892 | 0.683 | 2.877 | 2.867 | 11.091 | 8.795 |
| Storage proof job | 0.220 | 0.198 | 1.421 | 1.522 | 5.898 | 5.461 |
| Account proof service sum/block | 2,472.082 | 2,203.637 | 4,028.605 | 3,508.775 | 8,139.926 | 6,184.605 |
| Storage proof service sum/block | 1,033.099 | 958.672 | 1,948.530 | 2,246.094 | 5,557.456 | 5,237.513 |

The proof samples contain 3,155,278 account jobs / 3,453,823 storage jobs in old
mode and 3,047,061 / 3,373,544 in new mode. Account job mean duration decreases
from 1.445 ms to 1.246 ms; storage job mean decreases from 0.607 ms to 0.595 ms.
The per-block service sums account for both job duration and job count, but
include overlapping worker time.

### Persistence and sustained replay

`save_blocks` durations below are **seconds per batch**:

| Window | Mode | Batches | p50 | p90 | p99 | Mean |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| After warmup, completed during replay | old | 42 | 47.51 | 73.33 | 83.67 | 40.76 |
| After warmup, completed during replay | new | 57 | 15.15 | 79.21 | 92.55 | 35.62 |
| All batches, including shutdown | old | 54 | 47.51 | 77.99 | 94.36 | 41.42 |
| All batches, including shutdown | new | 69 | 15.15 | 83.71 | 110.49 | 38.11 |

New mode makes more, smaller batches, so its lower batch median does not imply
lower total persistence cost. The primary window averages 33.33 appended
blocks/batch in old mode and 25.35 in new mode.
Summed primary-window save duration divided by summed appended blocks is
**1.223 s/block old versus 1.405 s/block new (+14.9%)**.

Across all 600 blocks, including warmup and the final flush, average summed
persistence handler time per run is **745.56 s old versus 876.59 s new
(+17.6%)**. Both modes append exactly 600 blocks and fully persist the final
state. This corresponds to **1.243 versus 1.461 s per appended block**. These
are cumulative handler durations, not extra time to add to replay wall time;
persistence overlaps execution.

Measured replay throughput, including ordinary engine backpressure but excluding
shutdown, is **0.951 Ggas/s old versus 0.771 Ggas/s new (−18.9%)**. Client RPC
p99 reaches about 47–48 seconds in both modes because of persistence waits;
server validation p99 excludes those earlier waits.

### Repetitions

Each row scores the same 550 blocks after warmup. Replay duration runs from the
50th to the 600th completed submission, including forkchoice updates and
persistence backpressure.

| Run | Validation p50 (ms) | p90 (ms) | p99 (ms) | Mean (ms) | Replay (s) | Ggas/s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| old-1 | 125.20 | 149.05 | 382.10 | 135.04 | 574.32 | 0.975 |
| new-1 | 416.44 | 498.17 | 1206.84 | 418.53 | 713.87 | 0.784 |
| new-2 | 408.64 | 493.33 | 1234.26 | 415.93 | 746.67 | 0.750 |
| old-2 | 124.79 | 148.82 | 418.11 | 135.53 | 591.24 | 0.947 |
| old-3 | 125.84 | 148.56 | 413.22 | 136.17 | 600.53 | 0.932 |
| new-3 | 418.49 | 498.20 | 1212.54 | 419.76 | 716.43 | 0.781 |

Including warmup leaves the conclusion unchanged: all-block mean validation is
137.12 ms old versus 404.36 ms new.

All **3,600 payloads returned VALID**, all 600 state roots agree across all six
runs, every run used BAL execution and the sparse-trie state-root strategy,
and timeout/fallback counters remained zero. Each run shut down cleanly with
Finish 24,979,600 and no partial-state-trie lag. The final recovery restored
Finish and partial-state-trie to **24,979,000** and verified the promoted root
`0xc60084882e9a560a2679b26dbec4a5c7a6f5ac27a02985776706b82e45d7e254`.
`/schelk` is mounted with approximately **2.1 TiB available**. No benchmark
state was promoted.

## Method

Three runs per mode, in the order old, new, new, old, old, new. Each run replays
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
big-block persistence can drain. An initial old-table run hit the previous
five-second shutdown limit; it was excluded and repeated with the extended
timeout. Smoke runs are also excluded.

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

All benchmark artifacts are preserved outside the recovered volume at
`/home/ubuntu/state-trie-db-benches-20260924`. They include binary hashes and
machine/build details (`manifest.json`), corpus metadata (`corpus.json`), exact
commands, compressed node and bench reports, metrics, checkpoint verification,
the driver (`run.py`), and analysis (`analyze.py`). Per-run CSVs and the pooled
`results.json` retain the measurements behind the tables. Empirical latency
distributions are saved as `latency-distributions.png` and
`latency-distributions.pdf`, with their plotting script in `plot.py`.

See [the PoC documentation](state-trie-db.md) for the schema, migration, and
disk-space measurements.
