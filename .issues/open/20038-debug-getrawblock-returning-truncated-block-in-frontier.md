---
title: '`debug_getRawBlock` returning truncated block in Frontier'
labels:
    - A-rpc
    - C-bug
    - S-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.009448Z
info:
    author: SamWilsn
    created_at: 2025-11-28T20:14:16Z
    updated_at: 2026-01-16T02:16:52Z
---

### Describe the bug

The returned block is truncated. It should be 634 bytes long, but reth only returns 528 bytes.

### Steps to reproduce

```bash
curl --json \
    '{"jsonrpc": "2.0", "method": "debug_getRawBlock", "params": ["0xb443"], "id": 1}' \
    http://localhost:8545
```
Returns:

```json
{"jsonrpc":"2.0","id":1,"result":"0xf9020df90208a05a41d0e66b4120775176c09fcf39e7c0520517a13d2b57b18d33d342df038bfca01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d4934794e6a7a1d47ff21b6321162aea7c6cb457d5476bcaa00e0df2706b0a4fb8bd08c9246d472abbe850af446405d9eba1db41db18b4a169a04513310fcb9f6f616972a3b948dc5d547f280849a87ebb5af0191f98b87be598a0fe2bf2a941abf41d72637e5b91750332a30283efd40c424dc522b77e6f0ed8c4b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000860153886c1bbd82b44382520b8252088455c426598b657468706f6f6c2e6f7267a0b48c515a9dde8d346c3337ea520aa995a4738bb595495506125449c1149d6cf488ba4f8ecd18aab215c0c0"}
```

### Node logs

```text

```

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

```
Reth Version: 1.9.3
Commit SHA: 27a8c0f5a6dfb27dea84c5751776ecabdd069646
Build Timestamp: 2025-11-27T19:23:00.864611673Z
Build Features: jemalloc,otlp
Build Profile: maxperf
```

### What database version are you on?

```
Current database version: 2
Local database version: 2
```

### Which chain / network are you on?

mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

cargo build --profile maxperf

### Code of Conduct

- [x] I agree to follow the Code of Conduct
