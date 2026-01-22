---
title: Improve consistency check UX
labels:
    - A-observability
    - C-enhancement
    - inhouse
milestone: v1.11
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.00652Z
info:
    author: mattsse
    created_at: 2025-11-15T09:10:04Z
    updated_at: 2026-01-13T23:44:24Z
---

### Describe the feature

currently `consistency_check` at launch ensures that all the data in static files and db properly line up and eventually does some healing e.g. if the node was shut down between commits.

however this often returns hard to parse error messages with unclear instructions how to resolve.

ideally we can convert this into specific error variants that we can print instructions for.

this can happen with snapshots that mix and match data for example

### Additional context

_No response_
