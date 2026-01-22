---
title: Improve tx manager hash packing
labels:
    - A-tx-pool
    - C-debt
    - C-enhancement
assignees:
    - 0xKarl98
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.005814Z
info:
    author: mattsse
    created_at: 2025-11-13T08:38:35Z
    updated_at: 2025-11-13T08:58:34Z
---

### Describe the feature

this is currently enforced by a duration budget

https://github.com/paradigmxyz/reth/blob/7a599dc13018acc7f7d206581bf4cd4eb6368091/crates/net/network/src/transactions/mod.rs#L1629-L1640

and always allocates some capacity

https://github.com/paradigmxyz/reth/blob/7a599dc13018acc7f7d206581bf4cd4eb6368091/crates/net/network/src/transactions/fetcher.rs#L422-L429

all of this is a bit hard to read, and a bit suboptimal in general

https://github.com/paradigmxyz/reth/blob/7a599dc13018acc7f7d206581bf4cd4eb6368091/crates/net/network/src/transactions/fetcher.rs#L78-L84

## TODO
* create a more efficient implementation of txhash fetching
* all the packing logic should be testable in isolation


@0xKarl98 

### Additional context

_No response_
