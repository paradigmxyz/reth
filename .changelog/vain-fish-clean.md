---
reth-trie-sparse: minor
---

Added debug recording support to `ArenaParallelSparseTrie` behind the `trie-debug` feature flag. Introduced `record_initial_state` to capture post-prune trie state, added `Prune` variant and `short_key`/`state` fields to `ProofTrieNodeRecord`, and removed `remaining_keys` from `RecordedOp::UpdateLeaves` in favor of `proof_targets`.
