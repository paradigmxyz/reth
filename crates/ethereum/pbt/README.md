# PBT node integration

Build the optional backend with `cargo build --release --bin reth --features pbt`.
Default builds do not link the PBT library or enable its engine/storage integration.

Library users can enable `reth-node-pbt/cli` to reuse
`cli::{PbtArgs, PbtChainSpecParser}`. The executable only parses these options and
wires the configured PBT node into launch.

```sh
reth init --chain /path/to/genesis.json --datadir /path/to/data
reth node --chain /path/to/genesis.json --datadir /path/to/data --pbt
```

Set `config.binaryTrieTime` in the genesis file to the activation timestamp.
Use `0` for PBT from genesis. Amsterdam must be active at genesis for BAL replay.
`--pbt` conflicts with `--snap.v2`.

Add `--pbt.parallel` for parallel hashing of dense execution and BAL batches.
Sparse updates remain incremental. Hashing uses Reth's existing Rayon pool.

```sh
reth node --chain /path/to/genesis.json --datadir /path/to/data --pbt --pbt.parallel \
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

`--pbt.cache-max-bytes` bounds estimated full-state cache bytes (default 1 GiB, zero
disables caching). Shared nodes are counted separately in each cached snapshot.
Live native checkpoints have no artifact-decoder account, leaf or aggregate-code cap.
Untrusted imports keep their own budgets: `--pbt.import-max-accounts` (1,000,000),
`--pbt.import-max-leaves` (10,000,000), `--pbt.import-max-code-size` (16 MiB) and
`--pbt.import-max-total-code-size` (256 MiB). Byte limits are specified in bytes.

Cold execution-state restoration still materializes the complete in-memory state.
Native proof reads load only authenticated search paths. This backend remains
intended for reference execution and devnets.

`eth_getProof` is disabled with PBT. `pbt_getProof` is available on authenticated
RPC. See the [dev node](../../../examples/pbt-node/README.md) for migration
and checkpoint options.

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

Run `reth node --help` in a PBT-enabled build for all settings and their compatible
`PBT_*` environment bindings. Library callers pass `PbtConfig` to
`PbtNode::new_with_config`; library code does not read process environment.
Checkpoint pruning is automatic and preserves genesis and the retirement checkpoint.

The validation path overlaps auxiliary MPT work with execution until retirement.
Pre-fork shadow checks authenticate onboarding endpoints and the activation parent
instead of rebuilding the complete MPT for every warm parent read.

```sh
docker build --build-arg FEATURES=pbt -t reth-pbt:local .
```
