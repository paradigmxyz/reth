---
title: Implement searchTransactionsBefore and searchTransactionsAfter
labels:
    - A-rpc
    - C-enhancement
    - D-good-first-issue
assignees:
    - caglaryucekaya
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.979687Z
info:
    author: mattsse
    created_at: 2024-12-22T18:13:51Z
    updated_at: 2025-01-31T15:59:50Z
---

### Describe the feature

ref https://github.com/paradigmxyz/reth/blob/f791f393481d69e9bfed149888e85ffa3f3dc04f/crates/rpc/rpc/src/otterscan.rs#L293-L303

this is likely similar to

https://github.com/paradigmxyz/reth/blob/f791f393481d69e9bfed149888e85ffa3f3dc04f/crates/rpc/rpc/src/trace.rs#L243-L247

## todo
* implement this by looping over the range
* should enforce a sensible page size, lets start with 100 blocks

### Additional context

#3726
