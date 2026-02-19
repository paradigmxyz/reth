---
reth-trie-sparse: minor
reth-trie: patch
---

Removed `update_leaf` and `remove_leaf` from the `SparseTrie` trait, making them private methods on `ParallelSparseTrie` instead. Removed provider-based node revelation from leaf operations, simplifying the API by eliminating `TrieNodeProvider` generics from `update_leaf`, `remove_leaf`, and related internal methods. Also removed several now-unused higher-level methods (`update_account`, `update_account_leaf`, `remove_account_leaf`, etc.) from `SparseStateTrie`, and updated the witness computation to use `update_leaves` directly instead of the removed provider-based APIs.
