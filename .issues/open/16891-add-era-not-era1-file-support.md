---
title: Add ERA (not ERA1) file support
labels:
    - C-enhancement
    - M-prevent-stale
    - S-needs-triage
assignees:
    - lean-apple
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.98881Z
info:
    author: RomanHodulak
    created_at: 2025-06-18T13:29:36Z
    updated_at: 2025-12-08T15:50:45Z
---

### Describe the feature

Currently, we have Era1 [Spec](https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era1.md) - Execution layer historical block data before The Merge.

Now the goal is to also support Era [Spec](https://github.com/eth-clients/e2store-format-specs/blob/main/formats/era.md) - Stores data from the genesis of the Beacon Chain onwards. Can be used by Execution layer clients for history from The Merge onward, including historical block data.

There are already [existing hosts](https://eth-clients.github.io/history-endpoints/) for this that can be used for testing.

### Additional context

_No response_
