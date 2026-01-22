---
title: Store PeerKind in known-peers file
labels:
    - A-networking
    - C-enhancement
    - D-good-first-issue
assignees:
    - Soubhik-10
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.986497Z
info:
    author: mattsse
    created_at: 2025-04-16T09:13:53Z
    updated_at: 2025-12-03T18:25:47Z
---

### Describe the feature

currently on shutdown we store just the enodes:

https://github.com/paradigmxyz/reth/blob/b36fc954d26258ac727b5cc13b771524411e1001/crates/node/builder/src/builder/mod.rs#L731-L731

thereby stripping away context, such as whether the peer is trusted or not.


## TODO
* introduce a new datastructure that also groups them by PeerKind in the file
* update read and write functions

reading:
https://github.com/paradigmxyz/reth/blob/b36fc954d26258ac727b5cc13b771524411e1001/crates/net/network-types/src/peers/config.rs#L284-L298

writing
https://github.com/paradigmxyz/reth/blob/b36fc954d26258ac727b5cc13b771524411e1001/crates/node/builder/src/builder/mod.rs#L731-L731

this change should be backwards compat so we should also try this format when reading:

https://github.com/paradigmxyz/reth/blob/b36fc954d26258ac727b5cc13b771524411e1001/crates/net/network-types/src/peers/config.rs#L296-L296

### Additional context

_No response_
