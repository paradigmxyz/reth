---
title: Support additional eth_subscribe handlers
labels:
    - A-rpc
    - C-enhancement
assignees:
    - 0xKarl98
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.017873Z
info:
    author: mattsse
    created_at: 2026-01-05T15:21:30Z
    updated_at: 2026-01-05T15:23:46Z
---

### Describe the feature

this is currently limited to 

https://github.com/paradigmxyz/reth/blob/e8cc91ebc2fdfb4befdbc468b0a6af74cacdf123/crates/rpc/rpc/src/eth/pubsub.rs#L213-L213

it would be nice to support arbitrary eth_subscribe params, for which I think we'd need to change this to serde_json::Value and then also support a fallback handler for unknown params in

https://github.com/paradigmxyz/reth/blob/e8cc91ebc2fdfb4befdbc468b0a6af74cacdf123/crates/rpc/rpc/src/eth/pubsub.rs#L287-L287

that is like `dyn FnOnce(serde_json::Value) -> Result<BoxStream<Value>` for example

by default this would then result in an invalid params issue


@0xKarl98 

### Additional context

_No response_
