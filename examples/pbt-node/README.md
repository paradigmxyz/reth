# PBT dev node

```sh
cargo +nightly run -p example-pbt-node
```

Launches a temporary Ethereum dev node with a PBT genesis root and the BLAKE3
reference configuration. The normal EVM, transaction pool, state tables, and
Engine API are retained. PBT handles state commitment; MPT updates are maintained
until post-activation finality permits retirement. `PBT_KEEP_MPT=true` retains
MPT for archive use. The node prints its RPC addresses. By default its database and
checkpoints are temporary. Set `PBT_DATADIR` to retain both across restarts.

`eth_getProof` is disabled because MPT proofs cannot authenticate the PBT root.
Internal PBT proof generation/verification is available in `reth-trie-pbt`, and
the example exposes the versioned `pbt_getProof` method described below on
authenticated RPC only.

## Durable operation and migration

```sh
PBT_DATADIR=./pbt-data cargo +nightly run -p example-pbt-node
```

Use Ctrl+C or a graceful termination signal to stop the node and flush its
persisted state. Restart with the same directory and activation configuration.
Startup authenticates the persisted tip against its checkpoint and fails if the
checkpoint is missing or inconsistent. The directory pins both genesis and the
activation timestamp, so changing either requires a separate directory.
Once migration has completed, a valid tip checkpoint resumes without reconverting
or replaying from the old anchor. Migration settings can remain configured after
the original artifacts or MPT history become unavailable. A corrupt existing tip
still fails verification.
An OS lock allows one node process per directory. A populated checkpoint store
with a missing `chain-config` marker is rejected rather than assigned a new chain.

| Environment variable | Meaning |
|---|---|
| `PBT_DATADIR` | Persistent node directory; unset creates a temporary directory. |
| `PBT_ACTIVATION_TIMESTAMP` | Unsigned fork timestamp; defaults to `0` for PBT from genesis. |
| `PBT_MIGRATION_SNAPSHOT`, `PBT_MIGRATION_PREIMAGES` and `PBT_MIGRATION_ANCHOR` | Supply preimages and an anchor together; a snapshot is optional. Without it, the node converts its retained MPT state using changesets to reconstruct the anchor. With it, the node imports the snapshot. The node resolves the anchor in its own header chain, proves that it is an ancestor of local finality, requires it to precede activation, and performs dual verification before publishing the checkpoint. The anchor may be older than the latest finalized block, including when finality has crossed activation. |
| `PBT_PRUNE_BLOCKS` | Optional comma-separated block hashes to prune at startup. Every candidate must be a strict ancestor of the locally finalized header. Finalized, sibling and unrelated blocks are rejected before deletion. |
| `PBT_CATCHUP_MAX_BLOCKS` | Nonzero budget for anchor-to-tip BAL catch-up; defaults to 1024 blocks. |
| `PBT_CATCHUP_MAX_BAL_BYTES` | Nonzero aggregate raw BAL-byte budget for catch-up; defaults to 67108864 bytes (64 MiB). |

After migration import or conversion, the node walks its trusted local headers from the
selected finalized anchor to the persisted tip and replays BALs. Missing local
BALs are requested over `eth/71`, with three attempts of at most ten seconds
each and shutdown cancellation. Self-conversion also observes shutdown between
accounts and slots; missing pruned history fails anchor-root authentication. Peer responses are authenticated against the
local header before their bytes count against the catch-up budget. Every
parent link, BAL commitment and resulting MPT/PBT root is checked before the
corresponding checkpoint is published. Successfully replayed peer BALs are retained
in the local BAL store. Unavailable BALs, invalid commitments, bad roots and
exceeded budgets stop startup. The node does not discover or download artifacts.
Checkpoints store incremental, content-addressed trie nodes in RocksDB. Explicit
pruning preserves every retained root and reclaims unshared obsolete nodes.
It does not automatically enumerate or discard orphaned branches.

Anchor selection and missing-BAL retrieval follow the separation used by
[CPerezz's geth PBT implementation](https://github.com/CPerezz/go-ethereum/tree/fb9925472f8b68792dc9e24bad63b9ac74a4b248):
conversion and import authenticate an explicitly selected finalized anchor, and
the follower obtains missing access lists through the existing networking client.
Automatic snapshot catalogs and anchor discovery are not inferred from that reference.
The EIPs remain authoritative for artifact encoding and replay semantics.

## Experimental authenticated proof RPC

`pbt_getProof(blockHash, keys)` accepts a block hash and an array of raw,
hex-encoded tree keys. Derive account/storage/code keys with `reth-trie-pbt::key`.
The method is available only on the JWT-authenticated RPC server, using the same
authentication as the Engine API. It is absent from ordinary HTTP/WS/IPC RPC.

The response is a versioned experimental format, not a replacement for the
standard `eth_getProof` schema:

```json
{
  "version": 1,
  "blockHash": "0x...",
  "pbtStateRoot": "0x0000000000000000000000000000000000000000000000000000000000000000",
  "proofs": [{
    "key": "0x...",
    "value": null,
    "steps": [],
    "terminal": {"type": "empty"}
  }]
}
```

`steps` run from root to terminal. Each prefix is a sequence of relative path
bits, followed by the `right` selector and sibling hash. The terminal is
`empty`, `leaf` (full hex `key` and 32-byte `value`), or `branch` (relative
`prefix`, `left` and `right` hashes). A `null` value proves absence; otherwise
`value` is the raw 32-byte leaf. Reconstruct `Proof`, `ProofStep` and
`ProofTerminal` to verify with `Blake3Hasher`. Clients must compare
`pbtStateRoot` with a separately trusted commitment; before activation it is a
shadow root rather than the header's MPT root.

Requests accept 1–16 nonempty keys of at most 8192 bytes each, with a raw
parameter budget of 263168 bytes. Four blocking proof jobs may run at once;
additional requests fail immediately with error `-32005`. Invalid parameters
return `-32602`, and unavailable/corrupt checkpoints return `-32603`.

The shared `reth-node-pbt` tests cover transfers, contract deployment and overflow storage, competing
branches, reorgs, timestamp activation, BAL replay, early empty payloads,
missing-checkpoint failures, anchor onboarding, durable restart, finalized
pruning and proof responses. Staged backfill uses the normal download/execution
pipeline with PBT-aware Merkle verification and unwind checks. BALs are fetched and
replayed in ranges bounded by the catch-up limits; the limits reset for each range.
Until MPT retirement, the final replayed state must also match execution's computed
MPT root. Afterwards, execution still authenticates the BAL and PBT replay verifies
the canonical PBT header root. The node
does not support PBT-native snap sync. See the [PBT crate](../../crates/trie/pbt/README.md) for the
implemented surfaces and remaining production integration work.
