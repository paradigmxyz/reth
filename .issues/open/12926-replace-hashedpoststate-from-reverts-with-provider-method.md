---
title: Replace `HashedPostState::from_reverts` with provider method
labels:
    - A-trie
    - C-enhancement
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.979272Z
info:
    author: Rjected
    created_at: 2024-11-27T21:54:13Z
    updated_at: 2024-12-19T08:20:24Z
---

### Describe the feature

If we used a provider method to initialize the `HashedPostState`, we could actually remove the `DatabaseHashedPostState` trait, which does not really make sense. This would involve replacing `from_reverts` with a provider method.

### Additional context

_No response_
