---
title: Request impl debug_dumpBlock & debug_accountRange method
labels:
    - A-rpc
    - C-enhancement
    - M-prevent-stale
    - S-feedback-wanted
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.995301Z
info:
    author: Pana
    created_at: 2025-08-13T09:11:54Z
    updated_at: 2025-09-05T10:01:22Z
---

### Describe the feature

These two methods are used to debug account state, which will export user states. To implement these two methods, we need to develop logic for paginated traversal of both the state trie and the storage trie. How should I go about implementing them?

### Additional context

API doc of [debug_accountRange](https://geth.ethereum.org/docs/interacting-with-geth/rpc/ns-debug#debugaccountrange)

_No response_
