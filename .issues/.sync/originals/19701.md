---
title: 'perf: dynamic worker scaling based on available workers for multiproofs'
labels:
    - A-trie
    - C-enhancement
    - C-perf
    - S-needs-benchmark
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.005421Z
info:
    author: yongkangc
    created_at: 2025-11-13T01:55:17Z
    updated_at: 2025-11-13T14:33:41Z
---

Implement adaptive worker pool scaling for parallel proof workers. Currently workers are statically allocated (see #19700), causing over-provisioning during low load and under-provisioning during spikes (e.g., Xen storage node bursts). 


We should dynamically spawn/shutdown workers on demand based on avaliable workers, scaling between configurable min/max bounds to reduce thrashing.

This requires benchmarks and testing.

### Proposal 
- worker avaliability: if its >= 85% usage scale up, once back to 70% we can scale down back to normal
- pending multiproofs: if pending multiproof > 200 (scale up by 20%)

### Additional context
