---
reth-trie-common: minor
reth-trie: minor
reth-trie-parallel: minor
reth-engine-tree: patch
---

Moved `ProofV2Target`, `MultiProofTargetsV2`, and `ChunkedMultiProofTargetsV2` from `reth-trie-parallel::targets_v2` into a new `reth-trie-common::target_v2` module, making these types available at a lower level without pulling in the full parallel trie crate. Added a `multiproof_v2` method to `Proof` in `reth-trie` that generates a state multiproof using the V2 proof calculator with synchronous account value encoding.
