---
title: mdbx implementation tightly coupled with Reth's Tables
labels:
    - A-db
    - A-sdk
    - C-enhancement
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.970401Z
info:
    author: Vid201
    created_at: 2023-09-11T08:38:40Z
    updated_at: 2025-03-25T11:25:06Z
---

### Describe the bug

There are multiple checks like this:

`let table = Tables::from_str(T::NAME).expect("Requested table should be part of `Tables`.");`

in the mdbx implementation, making mdbx impossible to use in your project with different tables.

This was possible before commit `019ddd1edca70f8af4de8aa6447c29acbc46d836`.

### Steps to reproduce

Source: https://github.com/paradigmxyz/reth/blob/main/crates/storage/db/src/implementation/mdbx/tx.rs#L43
