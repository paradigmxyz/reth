---
title: Use state change size instead of block number to determine merkle threshold
labels:
    - A-staged-sync
    - A-trie
    - C-enhancement
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.987947Z
info:
    author: Rjected
    created_at: 2025-05-30T19:18:53Z
    updated_at: 2025-06-24T14:33:34Z
---

### Describe the feature

Right now we use number of blocks to restrict the merkle stage. This is meant to bound how much time and memory the merkle stage takes. With gas limits increasing, bounding the stage by number of blocks will become a bad heuristic for time and memory growth. Instead, we should use total storage / account changes to restrict the merkle stage.


This should only be completed after https://github.com/paradigmxyz/reth/issues/16173 is completed

### Additional context

_No response_
