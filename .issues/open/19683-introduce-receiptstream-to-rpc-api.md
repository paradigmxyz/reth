---
title: Introduce Receiptstream to rpc API
labels:
    - A-sdk
    - C-enhancement
    - M-changelog
assignees:
    - mablr
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.005255Z
info:
    author: mattsse
    created_at: 2025-11-12T15:19:10Z
    updated_at: 2025-12-07T15:00:14Z
---

### Describe the feature

followup to https://github.com/paradigmxyz/reth/issues/19681

we need to wire pending block aware streams to rpc

I think we can introduce a new eth trait for this like

https://github.com/paradigmxyz/reth/blob/abe6bf612580c8dd2640e02c4725be8398d0d622/crates/rpc/rpc-eth-api/src/helpers/call.rs#L53-L53

that is 

```
pub trait EthSubscriptions: RpcNodeCore
```

and provides ways to obtain streams 
e.g. for new receipts
like:

https://github.com/paradigmxyz/reth/blob/abe6bf612580c8dd2640e02c4725be8398d0d622/crates/rpc/rpc/src/eth/pubsub.rs#L378-L382

assigning @mablr 

### Additional context

_No response_
