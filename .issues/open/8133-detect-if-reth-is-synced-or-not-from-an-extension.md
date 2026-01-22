---
title: Detect if reth is synced or not from an extension
labels:
    - A-exex
    - A-sdk
    - C-enhancement
    - M-prevent-stale
    - S-needs-investigation
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.973769Z
info:
    author: ralexstokes
    created_at: 2024-05-06T22:25:18Z
    updated_at: 2025-05-06T14:51:01Z
---

### Describe the feature

I have a reth extension where part of its function is driven by payload events from the payload builder.

E.g. https://github.com/ralexstokes/mev-rs/blob/86bd5fee136889cccec75e3efd29269cc2cbaf50/mev-build-rs/src/auctioneer/service.rs#L352

When I launch the extension, the reth node is usually out of sync by at least a few blocks and sends payload attributes events for "stale" blocks. I'd like to try detecting if the node is synced or not and waiting on some kind of event that this is the case.

### Additional context

_No response_
