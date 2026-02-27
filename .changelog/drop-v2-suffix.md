---
default: minor
---

# Drop V2 suffix from trie types

Renames trie proof types to remove the `V2` suffix, now that legacy proof code paths
have been superseded:

- `TrieNodeV2` → `TrieNode`
- `BranchNodeV2` → `BranchNode`
- `ProofTrieNodeV2` → `ProofTrieNode`
- `DecodedMultiProofV2` → `DecodedMultiProof`
- `ProofV2Target` → `ProofTarget`
- `MultiProofTargetsV2` → `MultiProofTargets`
- `ChunkedMultiProofTargetsV2` → `ChunkedMultiProofTargets`
- `target_v2` module → `target`
- `trie_node_v2` module → `trie_node`
- Various `_v2` function/method suffixes dropped

Old conflicting types renamed with `Legacy` prefix (`LegacyDecodedMultiProof`,
`LegacyProofTrieNode`, `LegacyMultiProofTargets`). Alloy's `TrieNode` and `BranchNode`
are no longer re-exported from `reth_trie_common` via the `nodes::*` glob to avoid
name conflicts.
