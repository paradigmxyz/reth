---
reth-trie-common: minor
reth-trie: minor
---

Added `contains_range` method to `PrefixSet` for checking if any key falls within a half-open range. Added prefix set support to `ProofCalculator` via `with_prefix_set`, enabling stale cached hash invalidation and branch collapse detection when keys are inserted or removed; propagated storage prefix sets through `SyncAccountValueEncoder`.
