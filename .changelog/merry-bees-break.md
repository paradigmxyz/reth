---
reth-engine-primitives: minor
reth-engine-tree: minor
reth-node-core: minor
reth-trie-parallel: minor
---

Added `--engine.proof-jitter` CLI option behind the `trie-debug` feature flag. When set, each proof worker sleeps for a random duration up to the specified value before starting proof computation, useful for stress-testing timing-sensitive proof logic.
