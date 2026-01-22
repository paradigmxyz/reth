---
title: split `DatabaseProvider::insert_block` to isolate transaction-specific writing
labels:
    - A-db
    - C-enhancement
    - D-good-first-issue
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.975405Z
info:
    author: Rjected
    created_at: 2024-07-31T20:10:42Z
    updated_at: 2025-12-07T12:06:55Z
---

In the new persistence / DB model we will not be first writing transactions to DB and gradually moving to static files. Instead, we will be directly writing transaction data to static files.

The `insert_block` method by default writes to the transaction and headers tables:
https://github.com/paradigmxyz/reth/blob/02d25304f9c3352474a7f57e10467ae7a4038c1f/crates/storage/provider/src/providers/database/provider.rs#L3431

If we split the method, with the first being for transaction + header data, and the other method for everything else, we can use the second method instead of `insert_block` in the persistence task.

This may also be of use for the bodies stage:
https://github.com/paradigmxyz/reth/blob/0fece98b0529788b56b24e806046d585b0517ea9/crates/stages/stages/src/stages/bodies.rs#L115
