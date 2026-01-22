---
title: StorageChangeSets static files segment
labels:
    - A-db
    - C-enhancement
assignees:
    - Rjected
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 18682
synced_at: 2026-01-21T11:32:16.000178Z
info:
    author: Rjected
    created_at: 2025-10-14T00:48:48Z
    updated_at: 2026-01-21T10:24:03Z
---

### Describe the feature

Currently we do not append storage changesets to static files, we should take the same approach as we take in account changeset static files (https://github.com/paradigmxyz/reth/issues/18846) to support storage changesets 

https://github.com/paradigmxyz/reth/blob/59ace5892559f9f3ad461cf15a1c190bab76d427/crates/storage/db-api/src/tables/mod.rs#L448-L455

Closes RETH-152

### Additional context

_No response_
