---
title: debug_traceBlockByNumber with javascript tracer takes 200x longer on reth than erigon
labels:
    - A-rpc
    - C-bug
    - C-perf
    - S-needs-benchmark
    - S-needs-investigation
    - S-needs-triage
    - S-stale
assignees:
    - jenpaff
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.971441Z
info:
    author: imulmat4
    created_at: 2023-10-30T21:20:42Z
    updated_at: 2026-01-20T14:12:41Z
---

### Describe the bug

I have quite a few traces that I run on blocks using my erigon node. I was hoping to switch to Reth so I stood up a standard Reth node in docker.
Args:
ghcr.io/paradigmxyz/reth:v0.1.0-alpha.10 node --chain mainnet  --datadir /data/reth --metrics 127.0.0.1:9001 --log.directory /data/ --authrpc.addr 127.0.0.1 --authrpc.port 8551 --authrpc.jwtsecret /data/jwttoken/jwt.hex --http --http.addr 0.0.0.0 --http.port 8545 --http.api "eth,net,web3,debug,trace,txpool" --ws --ws.api "eth,net,debug,web3,trace,txpool" --ws.addr 0.0.0.0 --rpc-max-response-size 200
I noticed that my RPC calls on Reth are taking almost 200x longer. I will attach a test script In the repo steps that I was running to test with both erigon and Reth

### Steps to reproduce

[debug_test.txt](https://github.com/paradigmxyz/reth/files/13209923/debug_test.txt)
Please just run the above. Had to change the file format to txt from .py because of github. For my archive node its consistent at 230 seconds. 

### Node logs

_No response_

### Platform(s)

Linux (x86)

### What version/commit are you on?

reth Version: 0.1.0-alpha.10 Commit SHA: 1b16d80 Build Timestamp: 2023-09-26T19:06:39.318738288Z Build Features: default,jemalloc Build Profile: maxperf

### What database version are you on?

# reth db version
Current database version: 1
Local database is uninitialized


### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

I am not pruning

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [X] I agree to follow the Code of Conduct
