---
title: sanitize block gaps in eth_simulateV1
labels:
    - A-rpc
    - C-enhancement
    - D-good-first-issue
assignees:
    - iTranscend
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 15459
synced_at: 2026-01-21T11:32:15.984604Z
info:
    author: mattsse
    created_at: 2025-04-02T13:48:51Z
    updated_at: 2025-06-28T10:00:30Z
---

### Describe the feature

> All the other fields are computed automatically (eg, `stateRoot` and `gasUsed`) or kept as their default values (eg. `uncles` or `withdrawals`). When overriding `number` and `time` variables for blocks, we automatically check that the block numbers and time fields are strictly increasing (we don't allow decreasing, or duplicated block numbers or times). If the block number is increased more than `1` compared to the previous block, new empty blocks are generated in between.

ref https://github.com/ethereum/execution-apis/pull/484/files

in case the block overrides introduce gaps then we should fill that gap with empty blocks:

we must check this here:

https://github.com/paradigmxyz/reth/blob/7305c9ee0d1d9a5e3948d1a3df35fb400c7561f3/crates/rpc/rpc-eth-api/src/helpers/call.rs#L124-L124

here we must also check that the block number doesn't decrease.

if the number override introduces a gap, we must execute an empty `Calls` vec instead.

so we likely need to refactor this fn a bit so that we can reuse most of the execution logic

https://github.com/paradigmxyz/reth/blob/7305c9ee0d1d9a5e3948d1a3df35fb400c7561f3/crates/rpc/rpc-eth-api/src/helpers/call.rs#L170-L177

## TODO
* refactor simulateV1 so that a gap is filled with empty blocks, which is basically intermediary execution of empty statecalls vec:
https://github.com/paradigmxyz/reth/blob/7305c9ee0d1d9a5e3948d1a3df35fb400c7561f3/crates/rpc/rpc-eth-api/src/helpers/call.rs#L106-L106

### Additional context

_No response_
