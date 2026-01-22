---
title: debug_traceBlockByNumber error for a specific block on op-reth
labels:
    - A-rpc
    - C-bug
    - M-prevent-stale
assignees:
    - jenpaff
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.974861Z
info:
    author: mavileo
    created_at: 2024-07-08T17:18:47Z
    updated_at: 2025-03-13T12:46:03Z
---

### Describe the bug

debug_traceBlockByNumber on block 10418831 of Base chain run with op-reth returns:
`{"jsonrpc":"2.0","error":{"code":-32003,"message":"insufficient funds for gas * price + value"},"id":1}`

### Steps to reproduce

`curl http://localhost:8548 -X POST -H "Content-Type: application/json" --data '{"method":"debug_traceBlockByNumber","params":["0x9efa8f", {"tracer": "callTracer"}],"id":1,"jsonrpc":"2.0"}'`

### Node logs

_No response_

### Platform(s)

Linux (x86)

### What version/commit are you on?

reth Version: 1.0.0
Commit SHA: a4ba294fa5c6a6158a0c993ee453afcb4e57197c
Build Timestamp: 2024-07-06T18:46:38.222616870Z
Build Features: jemalloc,optimism
Build Profile: release

### What database version are you on?

Current database version: 2
Local database version: 2

### Which chain / network are you on?

Base

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

make install-op

### Code of Conduct

- [X] I agree to follow the Code of Conduct
