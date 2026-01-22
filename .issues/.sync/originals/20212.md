---
title: 'Tracking: Trie/DB Debugging Tools'
labels:
    - A-db
    - A-trie
    - C-tracking-issue
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.010752Z
info:
    author: yongkangc
    created_at: 2025-12-09T04:13:28Z
    updated_at: 2025-12-09T09:57:43Z
---

### Background

Debugging trie inconsistencies requires ad-hoc scripts to walk DB tables and decode raw hex output. We need CLI tools with decoded, human-readable output.

### Scope

* MDBX table walker with key bounds and DupSort subkey support
* Partial `repair-trie` for specific accounts or path prefixes
* Proof and trie node pretty printer
* Block-to-trie correlation to find candidate blocks

### Related Work

* #20092 - `reth db search-changesets` by @mediocregopher (draft, implements prefix-based changeset search with parallelization)
