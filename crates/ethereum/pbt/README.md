# PBT node integration

Enable the reference BLAKE3 PBT backend on a shared devnet with:

```sh
reth init --chain /path/to/genesis.json --datadir /path/to/data
reth node --chain /path/to/genesis.json --datadir /path/to/data --pbt
```

Use the same genesis, fork schedule, and BLAKE3 configuration as the other clients.
Set `config.binaryTrieTime` to `0` for PBT from genesis, or to the shared future
activation timestamp for an MPT-to-PBT devnet. Amsterdam must be active at genesis
so staged sync can authenticate and replay BALs. `--pbt` conflicts with `--snap.v2`;
PBT-native snap sync is not implemented. A configured PBT chain refuses to launch
without `--pbt`. Ordinary chains keep the existing MPT backend.

The binary parses the PBT genesis commitment for `init` and `node` consistently,
and includes `binaryTrieTime` in its EIP-2124 fork schedule. The runtime flag
installs PBT validation, payload building, durable checkpoints, and staged backfill.
Checkpoints live in the chain data directory's `pbt` subdirectory. Both commitment
domains remain available across activation and restart.

`eth_getProof` is removed while PBT is enabled. Experimental `pbt_getProof` is
available only on authenticated RPC. Optional `PBT_MIGRATION_*`, `PBT_PRUNE_BLOCKS`,
and `PBT_CATCHUP_*` environment settings retain the migration behavior described
in the [PBT dev node documentation](../../../../examples/pbt-node/README.md).

The node lifecycle, replay, persistence, and proof tests previously colocated in
the example now test this shared integration. Run `cargo nextest run -p reth-node-pbt`.
The example is a caller of this crate, as is the normal `reth` executable.

`testdata/geth-anchor-alloc.json` is the Geth-generated anchor allocation used by
[Nethermind's EIP-8347 fixtures](https://github.com/NethermindEth/nethermind/tree/ace54e0909a79c595cf4770020001dccca934cdb/src/Nethermind/Nethermind.State.Pbt.Test/Fixtures/Eip8347).
Its independent PBT root is checked during genesis configuration tests. Fixture
provenance records Geth commit `e31a37fb88c2b75c0897c033bd3f4279dee42268`.
