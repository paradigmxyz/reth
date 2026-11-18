# PBT node integration

Build with `cargo build --release --bin reth`. This enables PBT in the
regular Ethereum node. `config.binaryTrieTime` schedules activation.
Mainnet has no scheduled PBT activation.

Library users can enable `reth-node-pbt/cli` to reuse
`cli::PbtArgs` for performance, migration and storage settings. The normal Ethereum
chain parser, add-ons and backfill select commitments from the chain schedule.

```sh
reth init --chain /path/to/genesis.json --datadir /path/to/data
reth node --chain /path/to/genesis.json --datadir /path/to/data
```

Set `config.binaryTrieTime` in the genesis file to the activation timestamp.
Use `0` for PBT from genesis in tests. Bogota must be active by the
binary-tree fork. Replaying historical state from genesis still requires BALs
for that replay range; otherwise use the finalized migration anchor described
below. Snap/2 state synchronization is unsupported on binary-tree chains.

Add `--pbt.parallel` for parallel hashing of dense execution and BAL batches.
Sparse updates remain incremental. Hashing uses Reth's existing Rayon pool.

```sh
reth node --chain /path/to/genesis.json --datadir /path/to/data --pbt.parallel \
  --log.stdout.format json
```

`engine::pbt` INFO logs cover parent restoration, BAL replay, payload building,
and block validation. Payload and validation events include `phase`, `parallel`,
`accounts`, `leaves`, `pbt_update_ms`, `mpt_ms`, `checkpoint_ms`, `total_ms`, and
`maintain_mpt`. Durations are wall-clock milliseconds. `total_ms` excludes EVM
execution and parent restoration, which is logged separately. Compare matching
phases and `maintain_mpt` values when measuring runs. With overlapped validation,
`mpt_ms` measures remaining completion/fallback work after execution; the sparse-trie
task has its own metrics for its full background duration.

Payload candidates stay in memory; `candidate_cache_ms` measures that publication
and payload `checkpoint_ms` is zero. Accepted blocks sync their native checkpoint
and block reference before validation completes.

Checkpoints are stored in the chain data directory's `pbt` subdirectory. After a
post-activation block is finalized and persisted, an explicitly enabled retirement
(`--pbt.retire-mpt`) verifies its durable
PBT checkpoints, clears MPT node tables, and records retirement in MDBX.
Retirement survives restart. Unwinding below that boundary requires re-anchoring
or resynchronization. Flat execution state and history remain available.

MPT maintenance is retained by default. `--pbt.retire-mpt` irreversibly enables
retirement after post-PBT finality. Omitting it retains MPT, and cannot restore
an already retired MPT.

Finality maintenance reclaims obsolete checkpoints and unreferenced candidate roots.
`--pbt.checkpoint-history` retains 64 blocks below finality by default; genesis, the
retirement checkpoint and unfinalized branches remain available. Legacy references
whose headers and recorded heights are both unavailable are retained.
Native collection yields between sweep batches; cold readers and retirement
verification pin roots instead of holding the checkpoint writer lock throughout.
Checkpoint references and root records are pruned immediately. Full native-node
collection is deferred until 256 registered roots have been removed, amortizing
its whole-state scan. This counter survives restart; interrupted sweeps bypass
the threshold. Each maintenance tick also checks free space on the checkpoint
filesystem, even when finality has not advanced. Below 10 GiB free, outstanding
garbage is collected without waiting for 256 root removals; once that debt is
cleared, low space alone does not trigger repeated scans. Unreachable nodes use
disk space until collection. Library callers
can force reclamation with `SnapshotStore::collect`.

`--pbt.cache-max-bytes` bounds estimated full-state cache bytes (default 1 GiB, zero
disables caching). Shared nodes are counted separately in each cached snapshot.
Live native checkpoints have no artifact-decoder account, leaf or aggregate-code cap.
Untrusted imports keep their own budgets: `--pbt.import-max-accounts` (1,000,000),
`--pbt.import-max-leaves` (10,000,000), `--pbt.import-max-code-size` (16 MiB) and
`--pbt.import-max-total-code-size` (256 MiB). Byte limits are specified in bytes.

Steady-state cold execution updates can use authenticated disk paths. Startup
verification, BAL catch-up, migration and global code-ownership fallbacks still
materialize the complete in-memory state. Native proof reads load only their
search paths. This backend remains intended for reference execution and devnets.

Before activation, accepted blocks with a retained BAL use the normal MPT
validation path. A background worker follows canonical commits and reorgs to
publish their PBT shadow checkpoints. If it lags, activation recovers the exact
parent branch from retained execution results or authenticated BALs, then checks
the parent against its MPT commitment. Staged sync validates pre-BAL blocks
against their MPT commitments. To replay across activation, it needs a verified
checkpoint with BALs available from that checkpoint through the activation block.

MPT-only RPCs are disabled on a PBT-configured chain: `eth_getProof`,
`eth_getMultiProof`, `eth_getAccount`, `debug_accountAt`, `debug_executionWitness`,
`debug_executionWitnessByBlockHash`, `debug_intermediateRoots`, and
`debug_stateRootWithUpdates`. The binary Engine API does not advertise or generate
MPT execution witnesses. Ordinary state reads, tracing and binary payload methods
remain available. `pbt_getProof` is available on authenticated RPC. See the
[dev node](../../../examples/pbt-node/README.md) for migration and checkpoint options.

`--rpc.compute-state-root-for-eth-simulate` is rejected with PBT because that option
uses the MPT root provider. Ordinary simulation remains available.

```sh
cargo test -p reth-node-pbt
```

Migration uses `--pbt.migration-anchor` and `--pbt.migration-preimages` together.
Add `--pbt.migration-snapshot` to import a snapshot; otherwise retained local MPT
state is converted. `--pbt.catchup-max-blocks` bounds the onboarding range or staged
chunk (1024 by default), and `--pbt.catchup-max-bal-bytes` bounds each BAL chunk
(64 MiB by default). A full chunk ends replay preparation at its last fitting
block; only a single BAL larger than the configured budget is rejected.

Run `reth node --help` for all settings and their compatible
`PBT_*` environment bindings. Library callers pass `PbtConfig` to
`PbtNode::new_with_config`; library code does not read process environment.
Checkpoint pruning is automatic and preserves genesis and the retirement checkpoint.

The validation path overlaps auxiliary MPT work with execution until retirement.
Pre-fork shadow checks authenticate onboarding endpoints and the activation parent
instead of rebuilding the complete MPT for every warm parent read.

```sh
docker build -t reth-pbt:local .
```

Root timing logs include `prepare_ms` (parent selection, cold restoration and
auxiliary MPT preparation), `finish_ms` (root updates and checkpoint publication),
and their sum as `total_ms`. These measure commitment work; EVM execution and the
wait between preparation and finishing are excluded. The previous finishing-only
total is now exposed as `finish_ms`.
