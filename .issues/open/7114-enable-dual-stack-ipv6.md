---
title: Enable dual stack IPv6
labels:
    - A-networking
    - C-enhancement
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.972868Z
info:
    author: barnabasbusa
    created_at: 2024-03-12T15:15:27Z
    updated_at: 2024-09-24T11:04:57Z
---

### Describe the feature

Currently dual stack setups are not supported:
```sh
"node",
"--datadir=/data",
"--log.file.directory=/data/logs",
"--port=30303",
"--http",
...
"--nat=extip:147.182.168.104",
"--nat=extip:2604:a880:400:d0::2445:a001"                
```

It would be great if reth could advertise v4 and v6 addresses at the same time. 

```
error: the argument '--nat <NAT>' cannot be used multiple times

Usage: reth node [OPTIONS]

For more information, try '--help'.
```

### Additional context

_No response_
