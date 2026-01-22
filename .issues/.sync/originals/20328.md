---
title: send mined hashes to txmanager
labels:
    - A-networking
    - C-enhancement
assignees:
    - klkvr
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.013492Z
info:
    author: mattsse
    created_at: 2025-12-12T11:16:12Z
    updated_at: 2025-12-12T11:16:27Z
---

### Describe the feature

we can skip doing any work for recently mined txs and prevent processing them and evict them from the fetch queue as well

we can add a new command to

https://github.com/paradigmxyz/reth/blob/3c41b99599a941e9a33b8d19073520aefc9dd20d/crates/net/network/src/transactions/mod.rs#L97-L97

and spawn a monitoring task when we launch the network
we can add a simple cache for the recent x hashes


### Additional context

_No response_
