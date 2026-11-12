# PBT node integration

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
phases and `maintain_mpt` values when measuring runs.

Payload candidates stay in memory; `candidate_cache_ms` measures that publication
and payload `checkpoint_ms` is zero. Accepted blocks sync their native checkpoint
and block reference before validation completes.

Checkpoints are stored in the chain data directory's `pbt` subdirectory. After a
post-activation block is finalized and persisted, the node verifies its durable
PBT checkpoints, clears MPT node tables, and records retirement in MDBX.
Retirement survives restart. Unwinding below that boundary requires re-anchoring
or resynchronization. Flat execution state and history remain available.

Set `PBT_KEEP_MPT=true` before launch to retain MPT maintenance. This cannot
restore an already retired MPT.

Finality maintenance reclaims obsolete checkpoints and unreferenced candidate roots.
`PBT_CHECKPOINT_HISTORY` retains 64 blocks below finality by default; genesis, the
retirement checkpoint and unfinalized branches remain available. Legacy references
whose headers and recorded heights are both unavailable are retained.
Native collection yields between sweep batches; cold readers and retirement
verification pin roots instead of holding the checkpoint writer lock throughout.

`PBT_CACHE_MAX_BYTES` bounds estimated full-state cache bytes (default 1 GiB, zero
disables caching). Shared nodes are counted separately in each cached snapshot.
Live native checkpoints have no artifact-decoder account, leaf or aggregate-code cap.
Untrusted imports keep their own budgets: `PBT_IMPORT_MAX_ACCOUNTS` (1,000,000),
`PBT_IMPORT_MAX_LEAVES` (10,000,000), `PBT_IMPORT_MAX_CODE_SIZE` (16 MiB) and
`PBT_IMPORT_MAX_TOTAL_CODE_SIZE` (256 MiB). Byte limits are specified in bytes.

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
