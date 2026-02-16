# trie-debug

I want to add a new `trie-debug` feature to reth intended for debugging trie/state-root issues. This new feature should be propagated from the workspace all the way to the reth-trie-sparse crate.

The primary goal of the feature will be to record all mutating operations made against each ParallelSparseTrie, including the exact arguments made. When there is a SR mismatch or an incorrect trie updates generated (checked via `--engine.state-root-task-compare-updates`) then the operations which were made against all sparse tries will be written to a file for that block for later analysis.

## Sparse Trie

There should be a new module added to reth-trie-sparse containing the types and functionality used for tracking mutations. This module should only be included with the feature.

The ParallelSparseTrie, if the feature is enabled, should initialize the recorder as one of its fields. The recorder should be reset on every clear or prune call. The recorder should be called at the start of every method call which mutates the trie: update_leaves, reveal_nodes, update_upper_hashes, root. It is not necessary to record update_leaf/remove_leaf directly. These operations, including a clone of their arguments, should be recorded in-memory in the order they occurred.

## Engine

When the trie-debug feature is enabled, prior to clearing or pruning the sparse tries, the engine should take all the recorders out of the SparseStateTrie for the account and all revealed storage tries. If there is then a SR mismatch or an incorrect trie updates, the recorded operations should be recorded to a JSON file for the block in the pwd.
