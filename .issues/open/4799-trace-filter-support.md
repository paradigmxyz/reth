---
title: Trace Filter support
labels:
    - A-rpc
    - C-enhancement
    - C-perf
    - M-prevent-stale
    - S-needs-design
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.970681Z
info:
    author: mattsse
    created_at: 2023-09-26T13:31:39Z
    updated_at: 2025-04-18T08:55:03Z
---

### Describe the feature

tracking issue for `trace_fitler` which is very useful for indexing

ref https://github.com/paradigmxyz/reth/issues/3661#issuecomment-1735506192

`trace_filter` returns parity traces similar to `eth_getLogs` but for traces

https://github.com/paradigmxyz/reth/blob/7024e9a8e91e9546ffd20b5aae65e85888f591a6/crates/rpc/rpc-types/src/eth/trace/filter.rs#L9-L24

all filter fields can be checked via Transactions alone, if I understand the filter options correctly.

Supporting the block range shouldn't be too difficult, at worst this is essentially a batch of replay_block requests.

Not sure how to support address fields without limiting blocks, because atm we don't have indices for that

@tjayrush what is a usual trace_filter request?


### Additional context

_No response_
