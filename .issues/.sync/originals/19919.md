---
title: Add TransactionPoolExt::filter_pooled_txs(Fn(&) -> bool)
labels:
    - A-sdk
    - C-enhancement
    - D-good-first-issue
assignees:
    - AnInsaneJimJam
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.008318Z
info:
    author: mattsse
    created_at: 2025-11-23T14:19:51Z
    updated_at: 2025-11-30T20:30:04Z
---

### Describe the feature

this is intended as a cheaper variant for

https://github.com/paradigmxyz/reth/blob/9f3949cd35626f2685073103ccde4a759df72832/crates/transaction-pool/src/traits.rs#L310-L321

that uses a predicate to also filter txs

we should add this to this trait:

https://github.com/paradigmxyz/reth/blob/9f3949cd35626f2685073103ccde4a759df72832/crates/transaction-pool/src/traits.rs#L645-L645

for example we can improve this:

https://github.com/paradigmxyz/reth/blob/9f3949cd35626f2685073103ccde4a759df72832/crates/optimism/txpool/src/maintain.rs#L118-L118

## TODO
* add and impl fn
* update in maintain_transaction_pool_conditional

### Additional context

_No response_
