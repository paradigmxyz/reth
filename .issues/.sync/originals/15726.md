---
title: Collect sparse trie updates in a vector
labels:
    - C-enhancement
    - C-perf
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:15.986097Z
info:
    author: jenpaff
    created_at: 2025-04-14T14:31:39Z
    updated_at: 2025-08-18T15:59:39Z
---

### Describe the feature

## Description 
MultiProof::extend calls are expensive, so we can just collect them into a vector and reveal one by one.



### Additional context

started looking into this as part of https://github.com/paradigmxyz/reth/pull/15054
