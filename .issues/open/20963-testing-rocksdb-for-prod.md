---
title: 'Testing RocksDb for Prod '
labels:
    - C-enhancement
    - S-needs-triage
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20384
synced_at: 2026-01-21T11:32:16.018637Z
info:
    author: yongkangc
    created_at: 2026-01-12T16:31:00Z
    updated_at: 2026-01-21T10:21:30Z
---

### Describe the feature

- Restarts
- Consistency 
	- delete data from mdbx, and spring it up. 
- Run it with the nodes
- Integrate ChangeSets on Static Files to delete stuff
- test pipeline sync from block 0 
- reth bench compare

> unwind (with
> the feature branch), go again to the storage settings, and hten bench again. create a plan for this
> test
> 
> drop txn hash number, account history, storage history → reth bench

Closes RETH-86

### Additional context

_No response_
