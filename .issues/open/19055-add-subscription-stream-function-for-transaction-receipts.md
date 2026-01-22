---
title: Add subscription stream function for transaction receipts
labels:
    - A-rpc
    - C-enhancement
assignees:
    - futreall
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.000419Z
info:
    author: mattsse
    created_at: 2025-10-15T22:14:35Z
    updated_at: 2025-10-16T00:38:45Z
---

### Describe the feature

prep for https://github.com/alloy-rs/alloy/pull/2974

we can already implement a function for this, similar to:

https://github.com/paradigmxyz/reth/blob/5c19ce75805d8477b6e839cc0afd259c8f99da77/crates/rpc/rpc/src/eth/pubsub.rs#L87-L90

we need to leverage the conversion trait for the tx to receipt conversion

https://github.com/paradigmxyz/reth/blob/5c19ce75805d8477b6e839cc0afd259c8f99da77/crates/rpc/rpc/src/eth/pubsub.rs#L65-L67

https://github.com/paradigmxyz/reth/blob/5c19ce75805d8477b6e839cc0afd259c8f99da77/crates/rpc/rpc-convert/src/transaction.rs#L179-L187

basically just like

https://github.com/paradigmxyz/reth/blob/5c19ce75805d8477b6e839cc0afd259c8f99da77/crates/rpc/rpc-eth-api/src/helpers/block.rs#L126-L162

which we could actually convert to a helper fn in rpconvert first maybe so that this can be reused 

maybe @futreall

### Additional context

_No response_
