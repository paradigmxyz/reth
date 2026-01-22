---
title: Relax continuity invariant between database and static-files
labels:
    - A-db
    - A-static-files
    - C-debt
    - S-stale
    - inhouse
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.005987Z
info:
    author: joshieDo
    created_at: 2025-11-13T17:44:44Z
    updated_at: 2026-01-19T02:17:05Z
---

### Describe the feature

https://github.com/paradigmxyz/reth/blob/d77e4815c3f361f6ef850374ad37ae50e6191053/crates/storage/provider/src/providers/static_file/manager.rs#L1155-L1167

We no longer have any data type that exists simultaneously in static files and database, so this check no longer is necessary

### Additional context

_No response_
