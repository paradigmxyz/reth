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

Checkpoints are stored in the chain data directory's `pbt` subdirectory. After a
post-activation block is finalized and persisted, the node verifies its durable
PBT checkpoints, clears MPT node tables, and records retirement in MDBX.
Retirement survives restart. Unwinding below that boundary requires re-anchoring
or resynchronization. Flat execution state and history remain available.

Set `PBT_KEEP_MPT=true` before launch to retain MPT maintenance. This cannot
restore an already retired MPT.

`eth_getProof` is disabled with PBT. `pbt_getProof` is available on authenticated
RPC. See the [dev node](../../../examples/pbt-node/README.md) for migration
and checkpoint options.

```sh
cargo test -p reth-node-pbt
```
