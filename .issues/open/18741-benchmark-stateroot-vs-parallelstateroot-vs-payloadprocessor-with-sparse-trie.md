---
title: Benchmark `StateRoot` vs `ParallelStateRoot` vs `PayloadProcessor` with Sparse Trie
labels:
    - A-cli
    - A-trie
    - C-benchmark
assignees:
    - shekhirin
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.999399Z
info:
    author: shekhirin
    created_at: 2025-09-26T15:17:43Z
    updated_at: 2025-09-26T15:17:43Z
---

### Describe the feature

Should be a `reth-bench` subcommand that works on top of an existing datadir and benchmarks state root calculation using all three methods, according to the provided number of state transitions (makes sense only for `PayloadProcessor` that calculates in a streaming fashion), number of changed accounts, changed storage slots per account, % new accounts, % new storage slots.

### Additional context

_No response_
