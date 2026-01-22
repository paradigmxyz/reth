---
title: Expire pre-merge data on demand
labels:
    - A-db
    - A-sdk
    - C-enhancement
assignees:
    - RomanHodulak
milestone: v1.11
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 13186
synced_at: 2026-01-21T11:32:15.992616Z
info:
    author: mattsse
    created_at: 2025-07-04T13:18:36Z
    updated_at: 2026-01-13T23:45:40Z
---

### Describe the feature

currently we only expire pre-merge data on restart, this is bad UX because when syncing a fresh node a user needs to restart.

we can remove this data after we crossed the execution stage, or after we initially reached sync

also allow pruning data once we no longer need it.

Ideally this solution then also allows us to expire more data on a moving target, which could be integrated into the existing Pruner that kicks in on a regular basis


ref https://github.com/paradigmxyz/reth/blob/f8b678cf17b4a5ce4f5fa63f5514bc91bcad017f/crates/node/builder/src/launch/common.rs#L950-L981

https://github.com/paradigmxyz/reth/blob/f8b678cf17b4a5ce4f5fa63f5514bc91bcad017f/crates/prune/prune/src/pruner.rs#L27-L29


### Additional context

ref #17193
ref #18056
