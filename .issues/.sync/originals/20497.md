---
title: Never use historical provider when we can use latest
labels:
    - A-engine
    - C-bug
    - C-perf
assignees:
    - mattsse
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.015855Z
info:
    author: shekhirin
    created_at: 2025-12-18T19:30:55Z
    updated_at: 2025-12-18T19:31:01Z
---

### Describe the feature

https://github.com/paradigmxyz/reth/blob/4adb1fa5ac051f8d0e66b4e429b8b399b70bc33a/crates/engine/tree/src/tree/payload_validator.rs#L888-L896

there can be a race with persistence that will cause this to return a historical hash even though it's the same as latest tip

this will in turn make us do a `history_info` lookup and fall through to the plain state lookup https://github.com/paradigmxyz/reth/blob/4adb1fa5ac051f8d0e66b4e429b8b399b70bc33a/crates/storage/provider/src/providers/state/historical.rs#L209

### Additional context

_No response_
