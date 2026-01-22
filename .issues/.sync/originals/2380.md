---
title: Support reorged logs in eth_filter namespace
labels:
    - A-rpc
    - C-enhancement
    - M-prevent-stale
    - S-needs-design
assignees:
    - nkysg
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.968461Z
info:
    author: mattsse
    created_at: 2023-04-25T08:58:30Z
    updated_at: 2025-08-26T09:52:43Z
---

### Describe the feature

filter changes currently don't support reorged blocks in `eth_getFilterLogs` and `eth_getFilterChanges`

they are supported in eth pubsub because we can actively listen for new reorg events.

In order to support this in eth_filter, we need to track:
* a reasonable range of reorged blocks, likely with a separate task that writes reorged blocks to a shared map that's then checked when logs are served


## TODO
the best way to do this would be with a background task that listeners for reorged blocks updates relevant active filters

https://github.com/paradigmxyz/reth/blob/4b84b9935172f0951203b96b7d9f9359535bdbca/crates/rpc/rpc/src/eth/filter.rs#L561-L561

basically:
if there's a reorg, scan active filters and insert matching logs that should be remitted as reorged

### Additional context

_No response_
