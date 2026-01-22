---
title: 'Tracking: Move index tables out of mdbx'
labels:
    - A-db
    - C-enhancement
    - C-tracking-issue
    - inhouse
assignees:
    - fgimenez
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 18682
synced_at: 2026-01-21T11:32:16.008765Z
info:
    author: Rjected
    created_at: 2025-11-24T17:44:05Z
    updated_at: 2025-12-10T13:54:44Z
---

### Describe the feature

Right now we have a few index tables that grow quickly on high throughput chains, bloating the DB and causing lots of I/O on write. Moving these tables out of mdbx could help reduce write I/O.

Right now here are the largest index tables:
* `AccountHistory` (93G on base archive)
* `StoragesHistory` (846G on base archive)
* `TransactionHashNumbers` (325G on base archive)

Moving these out of mdbx requires a similar approach to static files, where we use a flag and entry in `StorageSettings` to determine whether to use mdbx for this data. Raw tx usage will need to be replaced with provider methods with good APIs.

### Additional context

_No response_
