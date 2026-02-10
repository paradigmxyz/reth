---
reth-trie: minor
reth-trie-parallel: minor
---

Added `root_node` and `storage_root_node` methods to proof calculators for efficient root-only calculations. These methods directly return the root node without requiring dummy targets, replacing the previous workaround of passing fake targets to proof generation.
