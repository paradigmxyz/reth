---
reth-trie-sparse: patch
---

Fixed a bug in `merge_subtrie_updates` where source insertions did not cancel destination removals (and vice versa), causing inconsistent trie updates accumulated across multiple `root()` calls without intermediate `take_updates()`. Added a test covering the cross-cancellation behavior.
