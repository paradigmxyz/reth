---
title: ExEx drops `ChainCommitted` notifications during reorg due to stale `finished_height`, causing missed re-execution and inconsistent state
labels:
    - A-exex
    - C-bug
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.006352Z
info:
    author: dhyaniarun1993
    created_at: 2025-11-14T13:18:48Z
    updated_at: 2026-01-20T02:17:02Z
---

### Describe the bug

https://github.com/paradigmxyz/reth/blob/26f575440d08227f20f8453b7526d33e3d869eb9/crates/exex/exex/src/manager.rs#L138-L151

This comparison becomes invalid during a reorg/revert, because `finished_height` is still pointing to the old chain while the ExEx has not yet processed the ChainReorged / ChainReverted event. This could lead to race condition.

### Steps to reproduce

NA

### Node logs

```text

```

### Platform(s)

_No response_

### Container Type

Not running in a container

### What version/commit are you on?

NA

### What database version are you on?

NA

### Which chain / network are you on?

Testing with OP devnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
