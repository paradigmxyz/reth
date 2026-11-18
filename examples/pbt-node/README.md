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

The example retains MPT maintenance by default. For migration, resource budgets,
parallel hashing, or explicit retirement, use the `reth` binary and its
`--pbt.*` arguments documented in [PBT node integration](../../crates/ethereum/pbt/README.md).
Library integrations pass those settings explicitly through `PbtConfig`.

`pbt_getProof(blockHash, keys)` is available only on JWT-authenticated RPC. It
accepts 1–16 raw hex-encoded tree keys and returns `version`, `blockHash`,
`pbtStateRoot`, and `proofs`. Each proof contains `key`, `value`, `steps`, and
`terminal`. Verify the proof against a trusted block commitment. Before
activation, the PBT root is a shadow commitment.

See [PBT node integration](../../crates/ethereum/pbt/README.md) for the normal
`reth` binary, parallel hashing, timing logs, and MPT retirement.
