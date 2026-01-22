---
title: Share a `HashedStorages` cursor among storage root calculations
labels:
    - C-enhancement
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.996565Z
info:
    author: hai-rise
    created_at: 2025-08-25T09:31:27Z
    updated_at: 2025-09-25T12:22:36Z
---

State root calculations are spending ~5.7% of time on creating new `tables::HashedStorages` database handles & cursors for each account's storage root calculation:
https://github.com/paradigmxyz/reth/blob/01f667c228ae752cc6a6e89e5f8c34c10daaf03b/crates/trie/trie/src/trie.rs#L620-L621

<img width="1107" height="792" alt="Image" src="https://github.com/user-attachments/assets/f69f3502-169f-4d00-ab3e-c6cd8dfcaf19" />

This is **very inefficient**. A single (sequential) state root run should share the same cursor for `tables::HashedStorages`, just like it already does for `tables::HashedAccounts`:
https://github.com/paradigmxyz/reth/blob/01f667c228ae752cc6a6e89e5f8c34c10daaf03b/crates/trie/trie/src/trie.rs#L165
