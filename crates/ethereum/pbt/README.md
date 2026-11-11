# PBT node integration

Enable the reference BLAKE3 PBT backend on a shared devnet with:

```sh
reth init --chain /path/to/genesis.json --datadir /path/to/data
reth node --chain /path/to/genesis.json --datadir /path/to/data --pbt
```

Add `--pbt.parallel` to hash dense execution and BAL update batches in parallel.
It requires `--pbt`; sparse updates stay incremental and produce the same commitments.
The arena backend is not enabled. Parallel hashing uses Reth's existing Rayon
thread pool.

PBT runs emit structured `engine::pbt` INFO events for parent restoration, BAL
replay, payload building, and block validation. Payload and validation events include `phase`,
`parallel`, `accounts`, `leaves`, `pbt_update_ms`, `mpt_ms`, `checkpoint_ms`,
`total_ms`, and `maintain_mpt`. `pbt_update_ms` includes state embedding and root
hashing; `checkpoint_ms` includes durable writes. `total_ms` covers that root-work
step, excluding EVM execution and separately logged parent restoration. Payload
and validation events are separate operations; compare matching phases and
`maintain_mpt` values when measuring serial and parallel runs.
`accounts` counts entries in the execution bundle or BAL, while `leaves` is the
resulting state-tree leaf count.
Durations are wall-clock milliseconds.

```sh
reth node --chain /path/to/genesis.json --datadir /path/to/data --pbt --pbt.parallel \
  --log.stdout.format json
```

Existing version-2 checkpoint markers are upgraded only after authenticating the
native PBT state. Corrupt checkpoints fail without publishing upgraded markers.

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
domains remain available across activation until a post-activation block is finalized.

After finality, the node waits for flat execution state to be persisted through that
block, re-verifies the durable PBT checkpoints of the finalized block and persisted
state tip, then atomically clears the account/storage MPT node tables and records
retirement in MDBX. In-flight MPT writes are discarded. Validation, payload building,
staged sync, and unwind then use PBT without maintaining MPT nodes. Retirement
survives restart; rollback below its finalized boundary requires re-anchoring or
resync. Flat account/storage state and history remain available to execution.

Set `PBT_KEEP_MPT=true` before launch to retain MPT maintenance and node storage
for archive use. It cannot restore an already retired MPT. Elapsed time or a fixed
block count alone never permits disposal: activation must be finalized, as required
by [EIP-8347](https://eips.ethereum.org/EIPS/eip-8347#transition-window).

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
