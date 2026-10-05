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
and RocksDB cache GiB (default 12).
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

The first 200,000 sampled accounts and their 600,000 slots form a stationary hot set. Two initialization
blocks reveal all hot keys. Each subsequent block selects `(1 - churn) * 100000` distinct hot accounts
uniformly, then selects the remaining accounts from outside this set, requiring their account and all
three slot paths to be unrevealed. Cold keys may be reused after they have been pruned. All blocks
update exactly 100,000 accounts and 300,000 slots. Actual pre-update witness misses are recorded.

A hot account's per-block selection probability is `p = (1 - churn) / 2`. For a run of `T` blocks,
`T * (1 - p)^L` bounds the probability that a particular hot account (and each of its slots) has any
untouched stretch of at least `L` blocks. Choose masking and buffer retention from this finite-horizon
bound, not from churn alone. Pruning only follows acknowledged persistence, so a slower writer can
increase retention. Check every hot key old enough to be pruned and audit 1,000 protected keys at each prune.
Also count any hot-key miss before an update after initialization, including distinct hot keys ever
evicted; both account and slot fractions must remain strictly below 0.1%.

`blocks.csv` reports task latency separately from workload preparation, persisted-update cleanup, and pruning. `root_ready_ms`
records the actual scheduled root completion; inputs are prepared before their fixed arrival,
and `input_late_ms` explicitly records any time the generator misses that arrival; `deadline_ms` also includes post-root pruning.
`root_ms + cleanup_ms + prune_ms` measures the isolated state/persistence service work, excluding
synthetic-input generation and witness auditing. Warmup and measurement are both paced. Fixed arrival
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
