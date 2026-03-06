---
reth-trie: patch
---

Fixed a potential panic in `ProofCalculator` by clearing internal computation state (`branch_stack`, `child_stack`, `branch_path`, etc.) after errors, preventing stale state from causing `usize` underflow panics when the calculator is reused. Added a test verifying correct behavior after simulated mid-computation errors.
