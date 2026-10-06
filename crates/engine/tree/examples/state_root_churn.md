# State-root throughput experiment

This destructive benchmark uses an expendable Ethereum mainnet snapshot. Recover before each
run and after the experiment; never use an active node's datadir. Synthetic empty blocks carry
state-root work and are not valid Ethereum execution.

```sh
cargo build --profile profiling -p reth-engine-tree --example state_root_churn \
  --features state-trie-rocksdb,reth-provider/state-trie-rocksdb,reth-provider/test-utils
state_root_churn sample /schelk/reth /path/corpus.bin 6000000
state_root_churn run /schelk/reth /path/corpus.bin 30 1000 100 30 /path/output 1000 6 4 100000
```

Arguments after `run`: datadir, corpus, churn percent, measured blocks, warmup blocks, retention
mask, output directory, then optional arrival period in milliseconds (1000), persistence threshold
(mask + 10), RocksDB cache GiB (12), and account updates per block (100000). Each account update
has three slot updates. The in-memory block buffer is five blocks.

The example compiles the production sparse-trie task directly and uses production proof v3
workers, update extraction, `save_blocks`, masked merging, SST staging, commit, and epoch pruning.
No EVM or execution overlay is involved. Proofs read the database directly on the preserved-trie
path. Pruning runs only after persistence completion is acknowledged, using the acknowledged
partial frontier. Exactly one save may be in flight.

The corpus contains existing storage-owning accounts with three existing slots each, sampled
using seeded random successor seeks. This is reproducible, but not uniform sampling of database
rows. For N account updates per block, the first 2N corpus accounts form the hot set. Every block
selects (1−churn)N distinct hot accounts and churn×N cold accounts from the remaining corpus.
Cold candidates must have unrevealed account and slot paths. Two initialization blocks reveal
the hot set. Nonces vary by epoch; full-width slot values hash (account, slot, epoch). Account
and slot churn are correlated, spanning N storage tries per block.

The next block's keys are selected before the current root. A bounded producer builds its input
state alongside current-block service, using immutable corpus data and no trie access. After
the current root, cold targets are prefetched through a warm-only production task. This must
preserve the root and emit no updates. Revealed paths receive access epochs, including ancestor
and storage-root epochs. Pruning therefore cannot immediately discard those prefetched paths.
Already-hot keys are not prefetched; rare hot misses use normal on-demand proof calculation.
Prewarming is serialized with the current root and can overlap persistence.

Timed runs do not perform witness audits, hot-eviction scans, or histogram collection. Only cold
candidate selection performs the lookups needed to define churn; candidates are not checked
again before prefetch. Last-written epochs for final verification are recorded by the input
producer. CSV output records block-level timings and actual persistence/commit duration.

Fixed arrivals never slide forward after a slow block. `deadline_ms` includes current root work,
next-block selection/prefetch, persistence cleanup, pruning, and any input lateness. `root_ms`
measures root calculation/extraction; `prewarm_ms` includes cold selection and prefetch.
`prepare_ms` is overlapping producer time and must not be added to those timings. The last block
has no successor to prewarm. No measured work is hidden by resetting the clock after warmup.

After measurement, release the trie and drain all pending updates. Verify the stored root,
sampled last-written account/slot values, and the root rebuilt with fresh database proofs. A
separate-process reopen should verify the same root. These checks are outside timed blocks.
Access-epoch/pruning behavior is covered separately by sparse-trie and state-root-task tests.

For capacity sweeps, vary N while retaining one-second blocks and the 1:3 account/slot ratio.
Scaling the hot pool to 2N preserves per-hot-key access probability p=(1−churn)/2. For T=1100
blocks, T×(1−p)^(mask+5) bounds a particular hot key's probability of any sufficiently long
untouched interval. Masks 18/19/21/25/30/47 for churn 0/5/10/20/30/50% keep this below 0.05%.
Performance runs rely on this bound; they do not claim observed eviction counts from disabled
audits. Keep CPU/RSS and native compaction logs, bracket a passing and failing rate with short
trials, then confirm the passing rate over 1000 measured blocks. Require bounded partial lag,
no one-second deadline misses, and successful final persistence checks. Report memory-limited
rates separately from latency failures and report the search resolution, not an exact limit.
