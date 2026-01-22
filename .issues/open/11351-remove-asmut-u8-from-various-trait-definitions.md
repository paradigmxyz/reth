---
title: Remove `+ AsMut<[u8]>` from various trait definitions
labels:
    - C-enhancement
    - M-prevent-stale
    - S-blocked
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.977156Z
info:
    author: DaniPopes
    created_at: 2024-09-30T17:47:47Z
    updated_at: 2025-04-18T10:20:46Z
---

### Describe the feature

Requiring `+ AsMut<[u8]>` is a major footgun, as it means two opposite things in the implementations of `BufMut` for `Vec<u8>` and `&mut [u8]`:
- `Vec<u8>` returns the written-to, initialized slice of bytes; this is used internally in e.g. `TransactionSignedNoHash` to fill the buffer with zeros while we still don't have the value we want to write at the start of the buffer (like flags for lengths)
- `&mut [u8]` returns the uninitialized spare capacity, which is not what's expected as seen above

I tried using this API for https://github.com/paradigmxyz/reth/issues/11296 but it fails because sometimes we use `&mut [u8]` as the `BufMut` which makes logic for the `Vec<u8>` usage incorrect.

We should remove the `BufMut + AsMut` pair of traits and write our own wrapper trait for `BufMut` that is implemented on Vec, SmallVec, ArrayVec, and slices that allows for indexing into the already-written-to buffer. This allows performance improvements like https://github.com/paradigmxyz/reth/issues/11296 by making it possible to go back and fill a value that was zero'd because the value is only available after encoding the entire struct

### Additional context

_No response_
