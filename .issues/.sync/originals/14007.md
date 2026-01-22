---
title: '`ExecutionOutcome` may return a wrong block count'
labels:
    - A-execution
    - C-bug
    - M-prevent-stale
assignees:
    - Aideepakchaudhary
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.98121Z
info:
    author: joshieDo
    created_at: 2025-01-27T15:39:29Z
    updated_at: 2025-03-11T10:31:38Z
---

### Describe the feature

We call the following method to get the block count of a `ExecutionOutcome` :

https://github.com/paradigmxyz/reth/blob/33bf34b2fb5967667705d9efab9ad3529b3c3f91/crates/evm/execution-types/src/execution_outcome.rs#L226-L229

However, we do not enforce/validate `self.receipts` during initialization. For example, if the `Receipts` is initialized as follows, `ExecutionOutcome::len()` will be 0 since `self.receipts` will be an empty vec.

https://github.com/paradigmxyz/reth/blob/33bf34b2fb5967667705d9efab9ad3529b3c3f91/crates/storage/db-common/src/init.rs#L246-L253

### Additional context

_No response_
