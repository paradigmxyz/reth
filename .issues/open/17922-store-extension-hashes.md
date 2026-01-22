---
title: Store extension hashes
labels:
    - A-trie
    - C-enhancement
    - C-perf
    - S-stale
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:15.995891Z
info:
    author: mediocregopher
    created_at: 2025-08-18T15:42:29Z
    updated_at: 2026-01-21T02:17:12Z
---

### Describe the feature

Try out storing extension node hashes in the db. When testing out the two PRs for #17571. I saw that we're pulling like 5x more branches out of the DB than we actually retain for proofs, specifically because we don't store the extension hashes. This could be a large saving.

### Additional context

Storing extension hashes would likely incidentally fix https://github.com/paradigmxyz/reth/issues/12129 as well
