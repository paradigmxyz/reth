---
title: debug_traceTransaction very slow for some transactions
labels:
    - C-bug
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.017725Z
info:
    author: phil-k1
    created_at: 2025-12-31T08:59:29Z
    updated_at: 2026-01-15T04:15:36Z
---

### Describe the bug

Calling `debug_traceTransaction` is very slow for some transactions.

### Steps to reproduce

For example, the following call takes more than 4s to complete on our Ethereum Mainnet Reth nodes (while it takes << 1s on Erigon):

```
% curl http://localhost:8545/ --request POST --header "Content-Type: application/json" --data '{"id": 1,"jsonrpc": "2.0","method": "debug_traceTransaction","params": ["0xf468d4e441c3d243de21519274229521113fc5b4f3cedc9aca4982f828999889", { "tracer": "callTracer", "timeout": "30s"}]}' -o /dev/null
```

### Node logs

```text

```

### Platform(s)

_No response_

### Container Type

Kubernetes

### What version/commit are you on?

v1.9.3

### What database version are you on?

2

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
