---
reth-trie-sparse-parallel: patch
---

Fixed parallel sparse trie to skip revealing disconnected leaves by checking parent branch reachability before inserting leaf nodes.
