---
reth-trie-sparse: patch
---

Refactored sparse trie node state tracking to use RLP nodes instead of hashes. Replaced `Option<B256>` hash fields with `SparseNodeState` enum that tracks either dirty nodes or cached RLP nodes with optional database storage flags. Added debug assertions to validate leaf path lengths and improved pruning logic to use node paths directly instead of path-hash tuples.
