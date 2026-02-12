---
reth: minor
reth-cli-commands: minor
reth-e2e-test-utils: minor
reth-ethereum-cli: minor
reth-node-core: minor
reth-optimism-bin: minor
reth-optimism-cli: minor
reth-prune: patch
reth-stages: patch
reth-storage-api: minor
reth-storage-db-api: minor
reth-storage-db-common: patch
reth-storage-provider: patch
---

Introduced `--storage.v2` flag to control storage mode defaults, replacing the `edge` feature flag with `rocksdb` feature. The new flag enables v2 storage settings (static files + RocksDB routing) while individual `--static-files.*` and `--rocksdb.*` flags can still override defaults. Updated feature gates from `edge` to `rocksdb` across all affected crates.
