# PBT adapters for Reth

Adapters for execution bundles, retained-state conversion, and durable PBT
checkpoints.

Features: `execution`, `migration`, `bal`, `conversion`, `persistence`, and
`parallel`. The parallel feature enables `execution::apply_bundle_parallel`;
node operators select it with `--pbt.parallel`. Disk overlays apply changes serially.
The payload and validation logs record `parallel_requested`, `disk_overlay`, and
whether the eager parallel batch implementation was selected (`parallel`).
Validation and payload building can use a pinned disk checkpoint on a cold cache
miss. A shared, 16,384-entry cache holds authenticated trie nodes. Each block applies
its own overlay, and only accepted block roots are published. Warm full-state
parents retain their existing update path. Migration, startup verification, the
activation boundary, and updates requiring global code ownership can still restore
the complete state. Reth continues to supply the EVM state and finalized bundles.

See [PBT node integration](../../ethereum/pbt/README.md) for node usage and logs.

```sh
cargo test -p reth-trie-pbt --all-features
cargo test -p reth-node-pbt
```
