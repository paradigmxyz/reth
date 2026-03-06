---
reth-trie: patch
---

Fixed handling of removed leaves in `ProofCalculatorV2` by masking out already-processed child nibbles when `uncalculated_lower_bound` advances past a branch, and fixed `MaskedTrieCursor` to clear the last remaining hash bit when all-but-one children of a branch are deleted. Added two regression tests and improved tracing instrumentation.
