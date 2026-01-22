---
title: '`no matching key/data pair found` when running `db stats` on older DB'
labels:
    - A-db
    - C-bug
assignees:
    - joshieDo
milestone: v1.11
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.006168Z
info:
    author: meyer9
    created_at: 2025-11-13T19:46:45Z
    updated_at: 2026-01-13T23:44:31Z
---

### Describe the bug

I'm getting an error fetching storage settings when running `db stats` on an old DB:

```
2025-11-13T19:45:24.641541Z  INFO reth::cli: Opening storage db_path="/data/reth-archive-new/db" sf_path="/data/reth-archive-new/static_files"
Error: failed to open the database: no matching key/data pair found (-30798)
```

Seems to be happening on this line: https://github.com/op-rs/op-reth/blob/200ad0a51c1325c47b22a21f55f0c485d59d11d0/crates/storage/provider/src/providers/database/mod.rs#L103

### Steps to reproduce

Run `reth db stats` with an old datadir.

### Node logs

```text

```

### Platform(s)

_No response_

### Container Type

Not running in a container

### What version/commit are you on?

latest main

### What database version are you on?

n/a

### Which chain / network are you on?

Base Mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
