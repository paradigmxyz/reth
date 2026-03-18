---
reth-trie-sparse: patch
---

Fixed another branch collapse edge case where `check_subtrie_collapse_needs_proof` incorrectly compared removal count against total update count (including `Touched` entries), causing it to skip proof requests for blinded siblings and panic when the subtrie emptied. Added a regression test covering the removals + `Touched` + blinded sibling scenario.
