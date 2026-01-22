---
title: Low validator effectiveness
labels:
    - C-bug
    - C-perf
assignees:
    - jenpaff
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.982793Z
info:
    author: chris13524
    created_at: 2025-03-18T04:34:14Z
    updated_at: 2025-06-05T10:23:41Z
---

### Describe the bug

I'm trying to use reth in my Rocket Pool validator, but I'm very often getting missed head votes, >0 inclusion distance, or sometimes even missed attestations. Was somewhere around 80% effective.

I switched to Nethermind a few weeks ago and my validator effectiveness immediately became basically perfect and has remained so. I just switched back to reth v1.2.2 (I thought maybe [this](https://github.com/paradigmxyz/reth/issues/5843) was my issue) but unfortunately the validator performance is still quite poor once again.

I would love to use reth (really just because it's Rust) in my validator. How can I help debug this?

An example slot is 11,286,668 which while had a 0-block inclusion distance had an incorrect head vote.

<img width="643" alt="Image" src="https://github.com/user-attachments/assets/4e9d611d-8b20-43d4-85d3-bbf8bef39796" />

<img width="371" alt="Image" src="https://github.com/user-attachments/assets/803a31db-6c0e-4400-933b-59c31e224c4c" />

### Steps to reproduce

Use reth & Nimbus in Rocket Pool node.

### Node logs

https://gist.github.com/chris13524/40075bd5a7055e50e77cb0cefd5c3cab

### Platform(s)

Linux (ARM), Linux (x86)

### Container Type

Docker

### What version/commit are you on?

reth Version: 1.2.2
Commit SHA: 2f4c509b3d01960f96abf2f9314b7d63db36977b
Build Timestamp: 2025-03-05T10:23:51.381756351Z
Build Features: asm_keccak,jemalloc
Build Profile: maxperf

### What database version are you on?

Got an error:

```
2025-03-18T02:00:16.617616Z  INFO Initialized tracing, debug log directory: /root/.cache/reth/logs/mainnet
Error: Datadir does not exist: "/root/.local/share/reth/mainnet"

Location:
    /project/crates/cli/commands/src/db/mod.rs:73:9
```

### Which chain / network are you on?

mainnet

### What type of node are you running?

Full via --full flag

### What prune config do you use, if any?

[prune]
block_interval = 5

[prune.segments]
sender_recovery = "full"

[prune.segments.receipts]
before = 11052984

[prune.segments.account_history]
distance = 10064

[prune.segments.storage_history]
distance = 10064

[prune.segments.receipts_log_filter.0x00000000219ab540356cbb839cbe05303d7705fa]
before = 11052984

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
