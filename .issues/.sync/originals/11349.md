---
title: Use `ArrayVec` in `Encode` implementations
labels:
    - C-perf
    - M-prevent-stale
    - S-blocked
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.976946Z
info:
    author: DaniPopes
    created_at: 2024-09-30T17:01:35Z
    updated_at: 2024-10-22T14:42:02Z
---

### Describe the feature

Some types like `StoredNibblesSubKey` have a maximum fixed size, which can benefit from using `ArrayVec<u8, N>` to avoid using heap memory. However, `BufMut` must be implemented for it, which requires a wrapper struct like `ArrayVecCompat<T, const N: usize>` which implements `BufMut` like for vec.

Blocked on https://github.com/paradigmxyz/reth/issues/11351 since we may use our own trait

### Additional context

_No response_
