---
reth-trie-sparse: minor
---

Fixed a bug in `ArenaParallelSparseTrie` where subtrie updates that would completely empty a subtrie were incorrectly dispatched to parallel workers instead of being processed inline, preventing correct branch collapse detection when blinded siblings are present. Refactored the `SparseTrie` test suite to accept a `fn() -> T` factory instead of requiring `T: Default`, enabling a new `arena_parallel_sparse_trie_always_parallel` test variant that exercises all tests with parallelism thresholds set to 1. Added `test_branch_collapse_multi_empty_subtries_blinded_remaining` to cover the case where removing multiple revealed leaves empties their subtries and leaves a single blinded sibling requiring a proof.
