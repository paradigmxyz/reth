---
title: Reuse evm tracing utils for debug_trace_block
labels:
    - A-rpc
    - C-enhancement
    - C-perf
assignees:
    - rose2221
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.991456Z
info:
    author: mattsse
    created_at: 2025-06-24T17:42:54Z
    updated_at: 2025-06-25T11:13:19Z
---

### Describe the feature

we still do:

https://github.com/paradigmxyz/reth/blob/48743963fc7a9445533391fb5057f42f51c84db8/crates/rpc/rpc/src/debug.rs#L112-L123

we recently added support for reusing the evm for block tracing:

https://github.com/paradigmxyz/reth/blob/48743963fc7a9445533391fb5057f42f51c84db8/crates/rpc/rpc-eth-api/src/helpers/trace.rs#L341-L342

which we can mirror for the debug_ api as well.

## TODO
* reuse better trace_block API for debug_ endpoint



### Additional context

_No response_
