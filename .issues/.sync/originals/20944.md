---
title: reth full node execution stage is extremely slow
labels:
    - C-bug
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.018485Z
info:
    author: Jinwei1987
    created_at: 2026-01-12T02:46:15Z
    updated_at: 2026-01-20T22:29:13Z
---

### Describe the bug

Running reth full node execution stage is very slow. 

<img width="4520" height="1584" alt="Image" src="https://github.com/user-attachments/assets/95341408-9ba1-4420-8d86-685abd75b949" />

### Steps to reproduce

reth node --full \
    --datadir data/reth \
    --authrpc.jwtsecret jwt.hex \
    --authrpc.addr 127.0.0.1 \
    --authrpc.port 8551 \
    --metrics 0.0.0.0:9001 \
    --http \
    --ws \
    --http.addr 0.0.0.0 \
    --ws.addr 0.0.0.0 \
    --http.api all \
    --ws.api all

### Node logs

```text
2026-01-12T02:38:29.131255Z  INFO Executed block range start=7820006 end=7820242 throughput="181.58Mgas/second"
2026-01-12T02:38:37.988627Z  INFO Received block from consensus engine number=24215701 hash=0x67dfbfcde919369148005841d488cb231b580bfe7fae937425e1914780bd790b
2026-01-12T02:38:39.160379Z  INFO Executed block range start=7820243 end=7820498 throughput="184.84Mgas/second"
2026-01-12T02:38:48.586407Z  INFO Received block from consensus engine number=24215702 hash=0x72bdb6974684537468d4a891396c4fa8d76d6f7f4c58c5b194cdec71a87a491c
2026-01-12T02:38:49.180287Z  INFO Executed block range start=7820499 end=7820738 throughput="172.10Mgas/second"
2026-01-12T02:38:51.798868Z  INFO Status connected_peers=100 stage=Execution checkpoint=7814540 target=24195518 stage_progress=9.51%
2026-01-12T02:38:59.199031Z  INFO Executed block range start=7820739 end=7820982 throughput="180.68Mgas/second"
2026-01-12T02:39:00.488241Z  INFO Received block from consensus engine number=24215703 hash=0xf341b37c17adfd5778719d38a541e6324946aa0de8a086d60b0b1339cb78ba34
2026-01-12T02:39:09.203896Z  INFO Executed block range start=7820983 end=7821212 throughput="177.00Mgas/second"
2026-01-12T02:39:12.559083Z  INFO Received block from consensus engine number=24215704 hash=0x0b52454da68b7039187089e7288baa40b5977244582f9532d246ab4aa416f37d
2026-01-12T02:39:16.798332Z  INFO Status connected_peers=100 stage=Execution checkpoint=7814540 target=24195518 stage_progress=9.51%
2026-01-12T02:39:19.221139Z  INFO Executed block range start=7821213 end=7821430 throughput="174.45Mgas/second"
2026-01-12T02:39:24.691500Z  INFO Received block from consensus engine number=24215705 hash=0xae93ce97376452f1f8a0726938f72f5c6cf588bc9df947ea704d49ad6afe5809
2026-01-12T02:39:29.234973Z  INFO Executed block range start=7821431 end=7821658 throughput="183.01Mgas/second"
```

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

Reth Version: 1.9.3
Commit SHA: 27a8c0f5a6dfb27dea84c5751776ecabdd069646
Build Timestamp: 2025-11-18T15:02:53.527360730Z
Build Features: asm_keccak,jemalloc,min_debug_logs,otlp
Build Profile: maxperf

### What database version are you on?

Current database version: 2
Local database version: 2

### Which chain / network are you on?

ethereum mainnet

### What type of node are you running?

Full via --full flag

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
