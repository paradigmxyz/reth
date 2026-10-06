# State-root churn experiment

This destructive benchmark operates on an expendable, fully persisted Ethereum mainnet snapshot.
Recover the snapshot before each run and after the experiment. Never run it against an active node.
The synthetic empty blocks intentionally describe state-root work, not valid Ethereum execution.

Build using `profiling`:

```sh
cargo build --profile profiling -p reth-engine-tree --example state_root_churn \
  --features state-trie-rocksdb,reth-provider/state-trie-rocksdb,reth-provider/test-utils
```

The example compiles the production sparse-trie task source directly. Production proof workers,
proof v3, trie-update extraction, `SaveBlocksInput`, `save_blocks`, masked merges, SST staging,
commit, and epoch pruning are used unchanged. No EVM, transaction execution, or execution overlay
is involved. Proofs read the database directly, as on the preserved-trie happy path. The in-memory
trie retains all updates beyond the acknowledged partial frontier.

```sh
state_root_churn sample /schelk/reth /path/outside-schelk/corpus.bin 6000000
state_root_churn run /schelk/reth /path/outside-schelk/corpus.bin 30 1000 100 30 /path/run
```

Arguments after `run` are datadir, corpus, churn percent, measured blocks, warmup blocks, masking
blocks, output directory, and optional arrival period in milliseconds (default 1000), persistence threshold (default masking + 10),
RocksDB cache GiB (default 12), and account updates per block (default 100000).
The in-memory block buffer is five blocks. Reducing the persistence threshold limits the batch size
without changing the minimum retention supplied by masking + buffer. This can reduce memory use
and cleanup spikes while preserving the hot-pruning probability bound.
As in the engine, persistence starts after the threshold is exceeded and only one save is in flight.
The final drain advances both durable frontiers to the synthetic head and verifies the stored root.

Sampling uses seeded random hashed-address seeks in legacy `HashedStorages`, retaining distinct
accounts with at least three existing slots and a `HashedAccounts` entry. Three random slot seeks
select existing slots, with three distinct fallback slots for small tries. Random successor sampling
is reproducible but is not an exactly uniform sample of table rows. No accounts or slots are created
or deleted. Each selected account's nonce changes, and its three selected slots receive new nonzero
values derived from the hash of `(account, slot, epoch)`. This prevents artificial compression
from repeating one small integer across all slots. Thus account and storage churn are correlated, and this workload has 100,000 storage tries
per block; it is not a workload concentrated in a few large contracts.

At the default rate, the first 200,000 sampled accounts and their 600,000 slots form a stationary hot set. Two initialization
blocks reveal all hot keys. Each subsequent block selects `(1 - churn) * 100000` distinct hot accounts
uniformly, then selects the remaining accounts from outside this set, requiring their account and all
three slot paths to be unrevealed. Cold keys may be reused after they have been pruned. At the default rate, blocks
update exactly 100,000 accounts and 300,000 slots; the optional update count scales both at 1:3. After each root, the next block's keys are selected and prefetched through a separate invocation of
that same production state-root task, before the current block's persistence-triggered prune.
Only keys with missing witnesses enter this warm-only invocation, which must preserve the root
and emit no persistence updates. The prewarm task refreshes the revealed paths without marking nodes dirty. Already-hot keys
retain their normal modification epochs and the configured eviction probability bound. Ancestors and storage roots receive the same access epoch. Consumption of a prefetched key in the next block must have zero missing account/slot witnesses,
including across pruning. Rare hot keys pruned after lookahead selection are counted as ordinary
consumption misses and handled by the current block's task.

Churn is measured before this prewarming, not at consumption. The warm phase is serialized after
the current root and charged to the current one-second interval; it can overlap background
persistence. This experiment does not overlap prewarming with the current root computation.
The first workload block is prefetched by the second initialization block. Actual pre-update
witness misses and prewarm cold-target counts are both recorded.

A hot account's per-block selection probability is `p = (1 - churn) / 2`. For a run of `T` blocks,
`T * (1 - p)^L` bounds the probability that a particular hot account (and each of its slots) has any
untouched stretch of at least `L` blocks. Choose masking and buffer retention from this finite-horizon
bound, not from churn alone. Pruning only follows acknowledged persistence, so a slower writer can
increase retention. Check every hot key old enough to be pruned and audit 1,000 protected keys at each prune.
Also count any hot-key miss before an update after initialization, including distinct hot keys ever
evicted; both account and slot fractions must remain strictly below 0.1%.

`blocks.csv` reports task latency separately from workload preparation, persisted-update cleanup, and pruning. `root_ready_ms`
records the actual scheduled root completion; inputs are prepared before their fixed arrival,
and `input_late_ms` explicitly records any time the generator misses that arrival; `deadline_ms` also includes next-block selection, prewarming, and post-root pruning.
Input value construction and witness auditing use parallel reads of the preserved trie. This keeps
the synthetic fixture generator from becoming the bottleneck at higher churn; the production root
task, persistence, pruning, selected keys, and generated values are unchanged.
`root_ms + prewarm_ms + cleanup_ms + prune_ms` measures recurring work after arrival, including
lookahead selection and prefetch target preparation. Current-block value construction occurs before
arrival when possible; any resulting lateness is included in the scheduled completion times. Warmup and measurement are both paced. Fixed arrival
timestamps do not slide forward when work falls behind. Deadline elapsed time includes any generator lateness and post-root pruning audits, making it a
conservative end-to-end harness measure. Arrival times are never reset at the warmup boundary. `saves.csv`
includes actual commit time and final drain. `metrics.csv` records per-block distributions of the
production histograms; these per-block percentiles must not be averaged to claim pooled percentiles.
Keep CPU/memory and RocksDB compaction logs alongside the run. Report deferred work and memory,
not just root timings, when assessing sustained capacity.

The provider uses the snapshot sender/transaction-lookup full-pruning settings and 64-block receipt
retention. These settings keep the synthetic empty blocks compatible with its existing static files.
At completion, verify sampled account/slot values against their last update epochs, then recompute
the same root with a fresh sparse trie and database proofs. An external reopen should also verify
the root after process exit.

`prewarm_ms` includes next-block selection, target construction, proof/reveal work, and access-epoch
refresh. `prune_cutoff` is zero unless persistence completion was acknowledged for that block.
Pruning is never performed solely because another block has arrived. The final measured block has
no successor to prewarm; exclude it from warm-duration percentiles. At 0% churn the smaller
persistence threshold of 6 (mask 18, buffer 5, RocksDB cache 4 GiB) can be compared with the older
threshold 28/cache 12 GiB run, but that comparison includes batching/cache changes.

For capacity sweeps, vary account updates per block while keeping a one-second arrival period and
three slots per account. The hot pool scales to twice the account update count, preserving the
per-key access probability and the same retention masks at every rate. The total corpus remains
six million accounts. Report account updates/s, slot updates/s, and their sum separately. Locate
an upper failing rate and a lower passing rate with short runs, then validate the passing rate over
1000 measured blocks. A passing rate requires zero full-cycle deadline misses, bounded partial
persistence lag, no memory-guard stop, and all final database checks. This is a measured interval,
not an exact hardware limit or a guarantee over an infinite run.
