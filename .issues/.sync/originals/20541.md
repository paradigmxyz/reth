---
title: Can't catch up to tip in 1.9.X
labels:
    - C-bug
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:16.016004Z
info:
    author: GigaSpider
    created_at: 2025-12-20T23:36:59Z
    updated_at: 2026-01-04T22:45:48Z
---

### Describe the bug

In the previous version 1.8, I was able to catch up to tip. Now in 1.9, if any significant time has elapsed where my node has been off or not syncing for whatever reason, I am unable to catch up to the tip again where my system gets caught in a loop that is constantly iterating over the pipeline stages, and staying within a few hundred blocks of the tip, but never being able to sync to it. 

### Steps to reproduce

1. Be atleast 24 hours out of sync
2. >reth node ...

### Node logs

```text

```

### Platform(s)

Mac (Apple Silicon)

### Container Type

Not running in a container

### What version/commit are you on?

v1.9.3

### What database version are you on?

2

### Which chain / network are you on?

mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

none

(archive node)

### If you've built Reth from source, provide the full command you used

reth node --authrpc.jwtsecret /Users/server1/desktop/lighthouse/jwt.hex --auth 0.0.0.0:8551 rpc.addr 0.0.0.0 --authrpc.port 8551 --datadir /Volumes/4tbA/reth --chain mainnet --http --ws --ws.addr 0.0.0.0 --http.addr 0.0.0.0 --ws.port 9546 --http.port 9545

### Code of Conduct

- [x] I agree to follow the Code of Conduct
