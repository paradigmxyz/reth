---
reth-engine-service: patch
reth-engine-tree: minor
reth-node-builder: patch
reth-trie-sparse: minor
reth-trie-sparse-parallel: minor
---

Added hot account tracking to smart-prune sparse tries during block building. Hot accounts (system contracts, major defi contracts, builders, fee recipients) are now preserved at full depth between blocks to avoid redundant proof fetching. Trie pruning now uses tiered preservation based on account hotness, with network-specific defaults for mainnet, Optimism, and Base.
