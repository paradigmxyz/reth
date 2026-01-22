---
title: Create E2E test to verify RocksDB provider functionality
labels:
    - A-db
    - C-enhancement
    - C-test
    - S-blocked
assignees:
    - fgimenez
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 19942
synced_at: 2026-01-21T11:32:16.014443Z
info:
    author: fgimenez
    created_at: 2025-12-16T17:14:54Z
    updated_at: 2025-12-16T17:16:24Z
---

Blocked by https://github.com/paradigmxyz/reth/issues/20384

Create a task to implement a simple E2E test for the newly integrated RocksDB provider. 

The test should involve:
* Create a node with RocksDB provider enabled
* Mining a few blocks.
* Querying transactions against those mined blocks.

### Additional context

_No response_
