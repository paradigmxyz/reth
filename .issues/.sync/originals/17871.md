---
title: 'perf: storage cursor reuse during EVM execution'
labels:
    - A-execution
    - C-enhancement
    - C-perf
    - S-stale
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:15.995522Z
info:
    author: rkrasiuk
    created_at: 2025-08-14T06:30:30Z
    updated_at: 2026-01-21T02:17:13Z
---

### Describe the feature

## Motivation

During EVM execution, as we enter the call frame we often do subsequent storage lookups for the same address. Current database state provider implementations (both latest and historical) create new cursor for each storage lookup and start searching the database for the given storage address.

## Description

Experiment with reusing the storage cursor and its position for subsequent storage lookups. See https://github.com/paradigmxyz/reth/tree/alexey/provider-cursor-reuse for early reference implementation.

### Additional context

- intial experiment: theres no perf gain yet. we should add metrics, check the cache hit rate. It could be that we are using the mutex, but they are not in contention. more investigation is needed. benched with `reth-bench-compare` on mainnet using a reth6 dev box. 
