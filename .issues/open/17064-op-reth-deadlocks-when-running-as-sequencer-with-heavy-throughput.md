---
title: op-reth deadlocks when running as sequencer with heavy throughput
labels:
    - A-block-building
    - A-op-reth
    - A-rpc
    - C-bug
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.991666Z
info:
    author: tchardin
    created_at: 2025-06-25T18:43:06Z
    updated_at: 2025-12-10T12:04:12Z
---

### Describe the bug

Around 1G Gas/s when it is the active sequencer on Redstone we've noticed Reth sometimes stalls and stops responding to any RPC requests including from the CL. Metrics also stops recording due to failed prometheus requests. There is no error message in the logs in debug mode as it just looks idle while all the requests just time out. Nothing seems out of the ordinary in Grafana until then with a number of open RPC connections in the low 100s.
Attaching a memory chart that looks like it's spiking a bit so maybe it's running out of memory? CPU chart doesn't seem to render anything.

<img width="680" alt="Image" src="https://github.com/user-attachments/assets/6a59f00a-fefd-4aa2-818d-a724c03a4fe5" />
<img width="679" alt="Image" src="https://github.com/user-attachments/assets/ca803218-c913-4f64-8f58-1006b42a5530" />

### Steps to reproduce

Unfortunately do not have steps to reproduce locally beyond trying to get the same level of throughput for a sustained duration. Happy to provide additional info if needed.

### Node logs

```text

```

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

127595e

### What database version are you on?

Current database version: 2
Local database version: 2

### Which chain / network are you on?

Redstone

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
