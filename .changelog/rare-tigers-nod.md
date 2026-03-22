---
reth-trie-sparse: patch
reth-engine-tree: patch
---

Removed the `skip_proof_node_filtering` flag, `revealed_account_paths`/`revealed_paths` tracking, and the `filter_revealed_v2_proof_nodes` function from the sparse trie implementation. Also removed the corresponding skipped-nodes metrics, simplifying the proof node reveal path to always pass nodes directly to the sparse trie without pre-filtering.
