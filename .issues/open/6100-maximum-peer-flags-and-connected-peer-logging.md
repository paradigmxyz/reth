---
title: Maximum Peer Flags and Connected Peer Logging
labels:
    - A-observability
    - C-enhancement
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.971841Z
info:
    author: SeaMonkey82
    created_at: 2024-01-17T01:53:16Z
    updated_at: 2025-12-04T10:40:30Z
---

### Describe the feature

Other execution clients have a generalized maximum peer flag rather than individual flags for outbound and inbound peers.  I prefer the ability to cap the overall peer count.  However, if the current implementation is preferable to others, and a generalized maximum peer flag isn't likely to be added, I suggest at least breaking down the connected peers in the client logging to inbound and outbound to give a better idea of which value needs a more restrictive limitation in order to reach an overall target number of peers.

### Additional context

_No response_
