---
reth-trie-sparse: minor
reth-engine-primitives: minor
reth-engine-tree: minor
reth-node-core: minor
reth-trie-common: patch
---

Added an arena-based sparse trie implementation (`ArenaParallelSparseTrie`) using `slotmap` arena allocation for node storage, enabling parallel subtrie mutation without per-node hashing overhead. Added `ConfigurableSparseTrie` enum to switch between the arena and hash-map implementations, and a `--engine.enable-arena-sparse-trie` CLI flag to opt in at runtime.
