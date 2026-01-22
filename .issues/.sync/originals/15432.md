---
title: 'Tracking: Snap sync support'
labels:
    - A-db
    - A-networking
    - A-sdk
    - C-enhancement
    - C-tracking-issue
    - M-prevent-stale
assignees:
    - Rjected
    - mattsse
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.9841Z
info:
    author: mattsse
    created_at: 2025-04-01T08:43:20Z
    updated_at: 2025-10-16T12:17:54Z
---

### Describe the feature

we already have all the snap protocol types:

https://github.com/paradigmxyz/reth/blob/0a56694308344bc2f94b6cd9bed4a2d7ae39814b/crates/net/eth-wire-types/src/snap.rs#L1-L1

introducing snap sync based syncing will require actual changes to:
* database
* networking
* ...

### Additional context

_No response_
