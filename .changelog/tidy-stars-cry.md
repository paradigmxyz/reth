---
reth-trie-sparse: patch
---

Fixed a bug where trie nodes could appear in both `updated_nodes` and `removed_nodes` simultaneously by removing entries from `removed_nodes` when a node is inserted as updated.
