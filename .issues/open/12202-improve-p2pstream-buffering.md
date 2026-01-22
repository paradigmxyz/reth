---
title: Improve P2PStream buffering
labels:
    - C-enhancement
    - M-prevent-stale
assignees:
    - Paulius0112
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.978308Z
info:
    author: mattsse
    created_at: 2024-10-30T15:58:39Z
    updated_at: 2025-09-10T14:37:18Z
---

### Describe the feature

we currently have a buffer:

https://github.com/paradigmxyz/reth/blob/ff9a42ae8fbf441f4fb9ca8dcea84c765b284721/crates/net/eth-wire/src/p2pstream.rs#L244-L245


## TODO
* remove buffer
* update how pings are processed, because if we now go through the Sink API, then this must be stored as options that we optionally need to keep around
* if we modify the poll logic in p2pstream then we also need to manually add a flush call at the callsite https://github.com/paradigmxyz/reth/blob/ff9a42ae8fbf441f4fb9ca8dcea84c765b284721/crates/net/network/src/session/active.rs#L569-L569


also ptal at the Sink docs if unfamiliar with the Sink API 

### Additional context

_No response_
