---
title: Very slow trace and debug rpc requests on BASE mainnet
labels:
    - A-execution
    - A-op-reth
    - A-rpc
    - C-perf
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.997923Z
info:
    author: lamgiahungaioz
    created_at: 2025-09-08T03:39:23Z
    updated_at: 2026-01-20T11:08:33Z
---

### Describe the bug

I want to get data about internal transactions and state changes in transactions on BASE mainnet. My current implementation calls trace_transaction and debug_traceTransaction to get those data. I notice that it takes around 5 seconds for 150-200 transactions, which in my use case is very slow. I have never had issue doing this on mainnet with Erigon. How should I resolve this?

Setup: R9 9900X, 128GB Ram, 2x8TB nvme

### Steps to reproduce

Run BASE mainnet archive node
Calls trace_transaction and debug_traceTransaction on each transaction



### Node logs

```text

```

### Platform(s)

Linux (x86)

### Container Type

Docker

### What version/commit are you on?

v1.6.0

### What database version are you on?

default

### Which chain / network are you on?

base mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

no prune

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
