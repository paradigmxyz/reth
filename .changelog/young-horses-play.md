---
reth-trie: major
reth-trie-db: major
reth-provider: minor
---

Added `MaskedTrieCursorFactory` and `MaskedTrieCursor` to handle prefix-set-based hash invalidation at the cursor layer, replacing the `DatabaseTrieWitness` trait abstraction. Removed `with_prefix_sets_mut` from `TrieWitness` and deleted `DatabaseTrieWitness` — callers should now wrap their cursor factory with `MaskedTrieCursorFactory` to apply prefix sets during witness/proof computation.
