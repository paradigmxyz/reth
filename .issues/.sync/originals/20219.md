---
title: 'feat(cli): partial repair-trie iteration'
labels:
    - A-cli
    - A-trie
assignees:
    - mediocregopher
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20212
synced_at: 2026-01-21T11:32:16.011216Z
info:
    author: yongkangc
    created_at: 2025-12-09T09:56:12Z
    updated_at: 2025-12-09T10:00:08Z
---

### Background

`repair-trie` always iterates the entire trie. No way to verify just one account's storage trie or a specific path prefix.

Parent: #20212

### Scope

* `repair-trie --account <addr>` to verify single account's storage trie
* `repair-trie --path-prefix <nibbles>` to verify trie subsection
