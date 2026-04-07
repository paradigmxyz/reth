---
reth-trie-sparse: patch
---

Fixed a panic in `ParallelSparseTrie::reveal_nodes` when a boundary node's upper parent is absent or non-branch (e.g. when an upper extension crosses the boundary). The code now skips gracefully instead of unwrapping. Added a regression test covering this case.
