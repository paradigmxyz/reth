---
reth-trie: patch
reth-trie-sparse-parallel: patch
---

Fixed caching of RLP node in SparseSubtrie root and optimized root hash computation to reuse cached lower subtrie hashes. Changed default witness builder to use ParallelSparseTrie for improved performance.
