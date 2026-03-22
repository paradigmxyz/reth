---
reth-trie-db: minor
reth-engine-tree: minor
---

Added `PendingChangeset` and `PendingChangesetGuard` to `ChangesetCache` so concurrent readers wait for an in-progress computation instead of falling back to the expensive DB-based path. The guard automatically cancels the pending entry on drop (e.g. task panic), ensuring waiters always make progress.
