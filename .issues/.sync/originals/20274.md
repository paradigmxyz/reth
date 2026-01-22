---
title: 'BAL: Payload Validator: Parallel Execution'
labels:
    - A-engine
    - A-execution
    - C-enhancement
    - C-perf
assignees:
    - mediocregopher
type: Task
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 18253
synced_at: 2026-01-21T11:32:16.01272Z
info:
    author: mediocregopher
    created_at: 2025-12-10T15:56:03Z
    updated_at: 2026-01-14T01:45:01Z
---

### Describe the feature

When the BAL is present on a block we should take advantage of it to enable parallel execution of txs during validation.

* Create a new implementation of `revm::Database` which accepts an existing `revm::Database`, a BAL, and a tx index, and serves data from the BAL where possible, falling back to the inner `revm::Database` when not.
    * NOTE: New implementation likely not needed, if [BalDatabase](https://github.com/bluealloy/revm/pull/3070/files#diff-f341afaf70d2d0052eff6c12b4b8ce44aaf2e4e91f736c7ad1dfafe66a279564R174) in revm gets merged.

* When the BAL is present on a block then, rather than executing txs in the main engine tree thread, we should spawn a configurable number of tasks and send txs to them over a channel.
    * For each tx, the task should use the BAL to construct an `revm::Database` using the new implementation (see first bullet). It should execute the tx, and return any errors to the main engine tree thread.

* The main engine tree thread should block until all tx tasks are finished, or until one of them results in an error. If none result in an error then the result from the state root task should be checked as normal. 

### Additional context

_No response_
