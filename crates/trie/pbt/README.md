# PBT adapters for Reth

Adapters for execution bundles, retained-state conversion, and durable PBT
checkpoints. The tree implementation is provided by `pbt`, pinned in the
workspace `Cargo.toml`.

Features: `execution`, `migration`, `bal`, `conversion`, `persistence`, and
`parallel`. The parallel feature enables `execution::apply_bundle_parallel`;
node operators select it with `--pbt.parallel`.

See [PBT node integration](../../ethereum/pbt/README.md) for node usage and logs.

```sh
cargo test -p reth-trie-pbt --all-features
cargo test -p reth-node-pbt
```
