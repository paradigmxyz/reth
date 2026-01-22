---
title: op-node -> op-reth disconnect (op-reth v1.5.0+)
labels:
    - A-engine
    - A-op-reth
    - C-bug
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.993111Z
info:
    author: web3-nodeops
    created_at: 2025-07-21T16:41:24Z
    updated_at: 2025-10-07T15:05:25Z
---

### Describe the bug

Running base image v0.12.6 / 7 (op-reth v1.5.0 / op-reth v1.5.1) sync'ing base (mainnet)

Periodically op-reth (v1.5.0, v1.5.1) errors, logging:

```
ts=2025-07-18T09:12:57.487850699Z level=warn target=libmdbx message="Long-lived read transaction has been timed out" open_duration=300.001270456s backtrace=None
```

This aligns with op-node being unable to talk to op-reth:

```
t=2025-07-17T11:15:57+0000 lvl=warn msg="Engine temporary error" err="temp: failed to update insert payload: failed to execute payload: Post \"http://<reth-url>/\": context deadline exceeded"
```

op-reth and op-node do not subsequently resume comms, the node stalls and op-reth needs to be restarted to start sync'ing

---

It appears related to high rpc activity (discussing with the base team directly, esp. high vol. eth_call can contribute to this error, which has been observed for other providers).

---

Downgrading base to image v0.12.5 (op-reth v1.4.3) and running for 48 hours, we observe no instances of op-reth / op-node stopping comms and stalling the node. With versions v0.12.5 / v0.12.6 instances of the error are sporadic, but can be as frequent as hourly.

### Steps to reproduce

Running base image v0.12.6 / 7 (op-reth v1.5.0 / op-reth v1.5.1) sync'ing base (mainnet)

Load node with rpc calls (esp. eth_call)

### Node logs

```text

```

### Platform(s)

Linux (x86)

### Container Type

Docker

### What version/commit are you on?

v1.5.0-61e38f9 / v1.5.1-dbe7ee9

### What database version are you on?

```
Current database version: 2
Local database version: 2
```

### Which chain / network are you on?

base

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
