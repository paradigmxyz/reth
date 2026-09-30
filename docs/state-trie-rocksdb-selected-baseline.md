# Selected overlapping RocksDB baseline

For the latest retained implementation and BAL measurements, see the
[complete-trie BAL optimization](state-trie-rocksdb-bal-optimization.md).
This page records the preceding baseline.

That baseline is the application source at `e7bade20a`, without the
uncommitted proof shortcut captured in `reth-bb-drainrocks`. It retains the
RocksDB read/cache/layout improvements, early signature recovery, exact overlay
hits, and the persistence/pruning shutdown barrier. The later `RAYON_NUM_THREADS`
override was reverted: the global pool again uses available CPU parallelism.
The ordinary proof-v3 calculator performs predecessor selection and ancestor
reads independently for each target; ancestor reuse and the successor-coverage
shortcut are absent.

Proof calculation and state preparation overlap BAL execution. Future performance
comparisons exclude runs that deferred this work until execution completed.

## Reference result

The captured `drainrocks` run validated 600 BAL blocks with matching roots,
persisted them, passed restart block 601, and recovered the baseline. After
excluding 50 warmup blocks, payload validation p50/p90/p99 was
116.1105/153.1799/228.90036 ms. Total saves including shutdown were 50.798742 s.
That binary included the experimental proof shortcut. These numbers are a
reference, not measurements of the restored source without that shortcut.

Artifacts are under:

```text
/home/ubuntu/state-trie-optimization-20260926/
  drainrocks-eight-rpcsmall-priority-isolated-vcache-proofside-cpupriority-chunk80-prunediag-600/
```

The saved `summary.json`, `node-command.json`, `bench-command.json`,
`observed-thread-counts.json`, `thread-affinities.json`, and
`thread-priorities.json` describe the measurement. The adjacent
`reth-bb-drainrocks.patch` captures the shortcut and shutdown changes against
the preceding source. The historical binary must not be used as the restored
baseline because it still contains the shortcut.

## Runtime settings

Build `reth-bb` with the `profiling` profile and `state-trie-rocksdb` feature.
Use these engine/cache settings for the selected configuration:

```text
--engine.account-worker-count 64
--engine.storage-worker-count 64
--engine.multiproof-chunk-size 80
--engine.persistence-threshold 50
--engine.memory-block-buffer-target 5
--engine.num-state-masking-blocks 30
--engine.state-root-task-timeout 0s
--rpc-cache.max-blocks 16
--rpc-cache.max-receipts 16
--rpc-cache.max-bals 16
--db.rocksdb-block-cache-size 8589934592
```

Both proof pools had 64 workers, BAL prefetch had 128 workers, and the BAL and
recovery pools each had 32 workers. The global Rayon pool used 32 workers on
dev-brian. The execution cache remained 4 GiB with code/storage/account weights
8/82/10. The trie SSTs had been rewritten to the retained 4 KiB/LZ4 layout.
The restored datadir still needs that rewrite before reproducing the benchmark;
restoring source does not change the promoted data or launch a migration.

The harness applied these host-specific settings to the node's threads after
the first warmup block, before the measured window:

| Threads | CPU affinity | Nice |
| --- | --- | ---: |
| Engine | 0 | -10 |
| Account/storage proof, BAL prefetch, native RocksDB background | 8–15, 24–31 | 0 |
| BAL execution, signature recovery, transaction iterator | 1–15, 17–31 | -5 |
| Other node threads, including global Rayon | 1–15, 17–31 | 0 |

These are benchmark settings, not new application defaults. The sender used
`--wait-for-persistence never`, no artificial submission delay, and 500 ms metric
scrapes. The reference also enabled pruning debug logs. No process, thread
affinity, database migration, recovery, promotion, or replay is started by this
source restoration.
