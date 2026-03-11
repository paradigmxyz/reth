---
reth-trie: patch
reth-trie-sparse: patch
---

Refactored test harness for sparse trie tests by extracting `TrieTestHarness` into a shared `reth-trie` test utility, replacing duplicated inline harness code across multiple test modules. Updated `proof_v2` return type to include an optional root hash, and converted `original_root` and `storage` from public fields to accessor methods.
