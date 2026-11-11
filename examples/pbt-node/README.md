# PBT dev node

```sh
cargo +nightly run -p example-pbt-node
```

Launches a temporary dev node with a PBT genesis root. The node prints its RPC
addresses. To retain its database and checkpoints across restarts:

```sh
PBT_DATADIR=./pbt-data cargo +nightly run -p example-pbt-node
```

Stop gracefully to flush state. Restart with the same genesis and activation
configuration; startup verifies the persisted checkpoint.

| Environment variable | Purpose |
|---|---|
| `PBT_DATADIR` | Persistent data directory; unset uses temporary storage. |
| `PBT_ACTIVATION_TIMESTAMP` | Fork timestamp; defaults to `0`. |
| `PBT_KEEP_MPT` | Set to `true` to retain MPT maintenance. |
| `PBT_MIGRATION_ANCHOR` | Local finalized ancestor selected for migration. |
| `PBT_MIGRATION_PREIMAGES` | Required with a migration anchor. |
| `PBT_MIGRATION_SNAPSHOT` | Optional snapshot; otherwise retained state is converted. |
| `PBT_PRUNE_BLOCKS` | Comma-separated block hashes; only strict ancestors of local finality may be pruned. |
| `PBT_CATCHUP_MAX_BLOCKS` | Maximum BAL catch-up range; defaults to `1024`. |
| `PBT_CATCHUP_MAX_BAL_BYTES` | Maximum BAL bytes per range; defaults to `67108864`. |

Migration authenticates the selected anchor and replays BALs to the persisted
tip. Missing BALs are fetched over `eth/71` and checked against local headers
before checkpoint publication. Missing history, invalid commitments, or exceeded
budgets stop startup.

`pbt_getProof(blockHash, keys)` is available only on JWT-authenticated RPC. It
accepts 1–16 raw hex-encoded tree keys and returns `version`, `blockHash`,
`pbtStateRoot`, and `proofs`. Each proof contains `key`, `value`, `steps`, and
`terminal`. Verify the proof against a trusted block commitment. Before
activation, the PBT root is a shadow commitment.

See [PBT node integration](../../crates/ethereum/pbt/README.md) for the normal
`reth` binary, parallel hashing, timing logs, and MPT retirement.
