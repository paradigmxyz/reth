---
title: Experiment with pointer-based sparse trie
labels:
    - A-trie
    - C-enhancement
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:15.99471Z
info:
    author: Rjected
    created_at: 2025-08-12T17:36:06Z
    updated_at: 2025-09-09T17:34:36Z
---

### Describe the feature

Right now we use a few hashmaps to implement the sparse trie (and parallel sparse trie), this leads to many hashmap lookups, we should have a parallel sparse trie that instead stores its internal representation using pointers. This might be more performant due to the lack of hashmap lookups

wip https://github.com/paradigmxyz/reth/tree/alexey/sparse-trie-boxes

### Additional context

_No response_
