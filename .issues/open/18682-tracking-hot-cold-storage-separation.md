---
title: 'Tracking: Hot/Cold storage separation'
labels:
    - A-db
    - A-static-files
    - C-debt
    - C-enhancement
    - C-tracking-issue
assignees:
    - yongkangc
milestone: v1.11
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.999012Z
info:
    author: shekhirin
    created_at: 2025-09-24T15:08:52Z
    updated_at: 2026-01-21T11:01:00Z
---

### Describe the feature

Currently, Reth uses two storage systems for all data: MDBX database for hot data, and Nippy Jar static files for cold data. It works the following way:
1. When new block is received, executed, and validated, we insert headers, transactions, and receipts into both database and static files.
2. When reading headers/transactions/receipts, we look in both database and static files, in case if the data is present only in database.
3. Pruner deletes the data from database, leaving it only in static files.

This doesn't make much sense now, but before we didn't write to static files directly, and instead were writing only to database, then `StaticFileProducer` was **copying** the data to static files, and `Pruner` removing it from database. Since the time we started writing to static files immediately on committing new block, we can simplify the whole storage system a lot.

The main goals of this tracking issue are:
1. Do not ever have duplicate data between database and static files. This simplifies the mental model of storage, and allows to look into only one storage in providers.
2. Move all append-only data to static files. This leaves only hot ever-changing data and indices in the database, making the access to historical data faster and the size smaller.

### Additional context

_No response_
