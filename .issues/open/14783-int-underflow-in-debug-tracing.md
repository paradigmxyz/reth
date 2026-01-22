---
title: Int underflow in debug tracing
labels:
    - A-execution
    - A-rpc
    - C-bug
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.982466Z
info:
    author: nerolation
    created_at: 2025-03-01T16:07:57Z
    updated_at: 2025-03-25T12:53:34Z
---

### Describe the bug

I stumbled across a potential int underflow bug when debug tracing transactions. The value for storage refunds gets above 1e19 which points towards an int underflow.
I documented it in this notebook:


### Steps to reproduce

https://github.com/nerolation/reth-debug_trace-bug-report

### Node logs

```text

```

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

haven't checked (found it a while ago and reported to reth team directly).
No access to the node anymore to re-check.

### What database version are you on?

same as the version/commit and would now take me a little longer to verify.

### Which chain / network are you on?

mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
