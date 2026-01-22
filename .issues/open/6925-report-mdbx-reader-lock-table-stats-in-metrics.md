---
title: Report MDBX reader lock table stats in metrics
labels:
    - A-db
    - A-observability
    - C-enhancement
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.972477Z
info:
    author: shekhirin
    created_at: 2024-03-01T22:27:04Z
    updated_at: 2025-04-18T08:57:43Z
---

### Describe the feature

We want this in metrics same as we have some stats for Freelist. It's helpful to be able to debug stagnant readers.

https://github.com/paradigmxyz/reth/blob/abab5a6938e0a4318f1bda9b398560ed6cc4a91e/crates/storage/libmdbx-rs/mdbx-sys/libmdbx/mdbx.h#L5338-L5350

### Additional context

_No response_
