---
reth-engine-primitives: patch
reth-engine-tree: patch
reth-node-core: patch
---

Removed `--engine.enable-arena-sparse-trie` CLI flag and made the arena-based sparse trie the default implementation. The hash-map-based `ParallelSparseTrie` variant is no longer selectable.
