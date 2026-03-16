---
reth-trie-sparse: patch
---

Refactored arena trie internals by adding a `BranchChildIdx::sibling()` helper, deduplicating `Index`/`NodeArena` type aliases, and replacing `is_empty()` with a `drop_root()` method. Fixed a bug where `cursor.pop()` was called before checking if the leaf was the root node, which could cause incorrect dirty-state propagation.
