---
reth-engine-primitives: patch
reth-engine-tree: patch
reth-node-core: patch
reth-trie-parallel: minor
---

Removed legacy proof calculation system and V2-specific configuration flags.

Removed the legacy (non-V2) proof calculation code paths, simplified multiproof task architecture by removing the dual-mode system, and cleaned up V2-specific CLI flags (`--engine.disable-proof-v2`, `--engine.disable-trie-cache`) that are no longer needed. The codebase now exclusively uses V2 proofs with the sparse trie cache.
