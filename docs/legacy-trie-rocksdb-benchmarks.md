# Legacy and complete trie backends

The `legacy-trie-rocksdb` feature replaces the durable legacy hashed/trie
backend for both execution and proof-v2. Execution keeps its BundleState
overlay; proof algorithms and legacy update types are unchanged. Account/slot
lookups and BAL batches use the same committed RocksDB snapshot as proof
cursors. `save_blocks` applies masked updates to RocksDB.

## Snapshot

The migration on dev-brian copied these records from the block 24,979,000
snapshot, retaining the old tables and both complete-trie backends:

| Legacy table | Records |
| --- | ---: |
| HashedAccounts | 382,731,390 |
| HashedStorages | 1,560,520,935 |
| AccountsTrie | 28,710,966 |
| StoragesTrie | 137,848,570 |

All 2,109,811,861 encoded records passed ordered source/destination digest
verification. Reopening each backend reproduced the baseline root
`0xc60084882e9a560a2679b26dbec4a5c7a6f5ac27a02985776706b82e45d7e254`.
Finish and partial-state-trie checkpoints were both 24,979,000. The verified
snapshot was promoted before replay. Available filesystem space after migration
was 1,979,735,007,232 bytes (1.80 TiB).

## Method

Each run replays the same 600 BAL blocks from
`txgen-big-blocks-mainnet-24979001-1000-1G.ndjson`, approximately 1 Ggas each.
Latency and proof-job distributions exclude the first 50 blocks. Persistence
includes every batch, including warmup and shutdown drain. Proofs overlap
execution; no deferred-proof run is included. Proof-v3 ancestor reuse and the
successor-coverage shortcut are absent.

Fresh legacy MDBX, legacy RocksDB and complete-trie RocksDB runs use profiling
builds on branch `mediocregopher/legacy-trie-rocksdb`. The complete-trie MDBX
column reuses the previous overlapping `overlaymdbx` run. All use 64 account
and 64 storage proof workers, chunk size 80, 128 BAL prefetch workers, 32 BAL
workers, 32 recovery workers and 32 global Rayon workers. The execution cache
is 4 GiB with code/storage/account weights 8/82/10; RocksDB's native cache is
8 GiB. Persistence threshold is 50, memory buffer 5, state masking 30, and
state-root timeout zero. RPC caches are limited to 16 blocks/receipts/BALs.

The engine uses CPU 0 at nice -10. Other workers exclude CPUs 0 and 16.
Proof, BAL prefetch and RocksDB background threads use CPUs 8–15 and 24–31;
BAL execution, recovery and transaction-iterator threads use nice -5. Both
RocksDB schemas use the retained 4 KiB/LZ4 table layout. The sender has no
artificial pacing and uses `--wait-for-persistence never` with 500 ms scrapes.

`State-root wait` is the wait after execution, not total concurrent root work.
Proof-job durations describe individual jobs, not wall time for a whole block.
Batch counts and total persistence time accompany `save_blocks` percentiles
because batch sizes can differ.

These are individual runs, not estimates with confidence intervals. The reused
MDBX complete-trie run occurred earlier on the same host. Different persistence
speeds change the durable frontier and overlay size during replay, so proof-job
counts need not be identical even with identical blocks and chunk sizes.

## Results

Each cell is **p50 / p90 / p99**, in milliseconds except where noted.

| Metric | MDBX + legacy | RocksDB + legacy | MDBX + new (reused) | RocksDB + new |
|---|---:|---:|---:|---:|
| Payload validation | 105.461 / 130.580 / 356.222 | 132.117 / 188.651 / 265.594 | 109.871 / 131.581 / 357.365 | 121.023 / 161.327 / 251.302 |
| Execution | 90.514 / 104.819 / 317.965 | 96.464 / 122.680 / 207.127 | 87.776 / 104.641 / 324.858 | 96.548 / 121.933 / 214.053 |
| State-root wait | 0.117 / 19.372 / 60.757 | 23.934 / 64.152 / 108.395 | 12.328 / 24.873 / 35.367 | 11.820 / 31.175 / 53.311 |
| Account proof job | 14.198 / 42.434 / 59.545 | 20.113 / 70.050 / 99.447 | 8.943 / 21.408 / 36.587 | 14.232 / 29.863 / 49.567 |
| Storage proof job | 0.261 / 3.738 / 21.548 | 0.556 / 5.689 / 27.688 | 0.099 / 1.797 / 13.084 | 0.303 / 3.795 / 19.001 |
| save_blocks (s) | 54.591 / 76.497 / 94.015 | 1.386 / 1.671 / 1.964 | 27.846 / 59.338 / 74.048 | 1.430 / 1.740 / 2.152 |
| Total save_blocks (s) | 941.572 | 51.234 | 542.589 | 52.712 |
| Save batches | 18 | 36 | 17 | 36 |
| Replay throughput (blocks/s) | 0.672 | 4.699 | 1.311 | 4.981 |

Legacy RocksDB increases payload validation p50 by **25.3%** and execution p50
by **6.6%**, while reducing total `save_blocks` time by **94.6%**. Replay
throughput is **6.99×** higher. Persistence-wait p99 falls from **53.24 s** to
**112.71 ms**. It is a persistence/throughput improvement, with a median
validation-latency regression. Payload and execution p99 improve in this run.

Within RocksDB, the complete schema reduces payload p50 by **8.4%**, account
proof-job p50 by **29.2%**, and storage proof-job p50 by **45.5%** relative to
the legacy schema. Execution p50 is effectively unchanged; total save time is
similar (52.71 vs 51.23 s). The complete schema is the faster RocksDB option for
payload validation here. Legacy MDBX has the lowest payload p50 of these runs.

Every fresh run matched all 600 reference roots, fully persisted block 24,979,600,
and validated and persisted block 24,979,601 after restart. The interval audit
found proof/execution overlap in all 550 measured blocks of each legacy run and
549 of 550 in the complete-trie RocksDB run; overlap was enabled throughout,
with no proof/execution barrier. No state-root fallback, timeout, or execution
trie-overlay computation occurred. Final recovery restored and
rechecked the promoted block 24,979,000 baseline, retaining all four backends.

Implementation checks passed 288 default provider/overlay tests, five targeted
legacy tests, five complete-trie RocksDB tests, and two root-deletion/snapshot
regression tests. Workspace all-feature Clippy, legacy-feature Clippy, nightly
formatting, zepter and TOML linting passed.

## Provenance

Source commits are `e8ba99f20` for legacy MDBX and complete-trie RocksDB, and
`f5572e14f` for legacy RocksDB. The latter fixes account-root deletion parity
only in the legacy RocksDB writer. Their parent is the restored overlapping
baseline `d4e7ddf31`. The captured binary version string retains cached parent
build metadata; `binaries.json` records actual source, features and SHA-256.

Artifacts, commands, migration digests, binary hashes, metrics, per-block CSVs,
thread audits and logs are under:

```text
/home/ubuntu/legacy-trie-rocksdb-20260928/
```

The reused complete-trie MDBX artifacts are under:

```text
/home/ubuntu/state-trie-optimization-20260926/
  overlaymdbx-rpcsmall-priority-isolated-vcache-proofside-cpupriority-chunk80-600/
```
