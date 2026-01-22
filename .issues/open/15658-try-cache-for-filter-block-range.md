---
title: Try cache for filter block range
labels:
    - A-rpc
    - C-perf
    - D-good-first-issue
assignees:
    - 07Vaishnavi-Singh
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.985713Z
info:
    author: mattsse
    created_at: 2025-04-10T09:02:20Z
    updated_at: 2025-12-03T18:30:11Z
---

### Describe the feature

we now have

https://github.com/paradigmxyz/reth/blob/3cf0d0d75b331932f2886b029addf83b8b47a19d/crates/rpc/rpc-eth-types/src/cache/mod.rs#L216-L220

we can use this to fetch the cached chain in reverse and optimize this range query:

https://github.com/paradigmxyz/reth/blob/3cf0d0d75b331932f2886b029addf83b8b47a19d/crates/rpc/rpc/src/trace.rs#L273-L280

## TODO
* fetch cached chain
* adjust offsets `start..chain.len()-(end -start)`
* fetch missing piece

### Additional context

_No response_
