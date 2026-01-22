---
title: Starting the node providing a --config file (and nothing else) ignores the datadir explicitly given there.
labels:
    - A-cli
    - C-bug
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.972141Z
info:
    author: cfc-ctf
    created_at: 2024-02-05T21:10:41Z
    updated_at: 2025-08-26T09:52:39Z
---

### Describe the bug

Starting the node providing a `--config` file (and nothing else) ignores the `datadir` explicitly given in said file, in my case a toml file exactly as per the one that reth creates while sending in parameters at start — whether in the command line or from `reth.service`.
Instead, the hardcoded dir is used. 

May I suggest that you either:
**a)** do not hardcode any data folder, but instead force users to always provide a datadir path,
…or…
**b)** honor the `--config` file provided [while starting], fetching all parameters, including `datadir` from that provided toml file 

### Steps to reproduce

1. Starting the node providing a `--config` file (and nothing else) ignores the datadir explicitly given there.
2. at this point, reth ignores the `datadir` specified in that toml.
3. actually, I believe more parameters are ignored, because I never succeeded in properly starting reth with something like `reth node --datadir /my-data-path --config /my-config-path/my-config.toml`

### Node logs

```text
N/A
```


### Platform(s)

Linux (x86)

### What version/commit are you on?

tried with alpha14 and alpha16

### What database version are you on?

Started in alpha14, now running alpha16

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

RUSTFLAGS="-C target-cpu=native" cargo build --profile maxperf --features jemalloc,asm-keccak

### Code of Conduct

- [X] I agree to follow the Code of Conduct
