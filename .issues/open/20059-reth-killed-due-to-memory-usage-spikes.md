---
title: Reth killed due to memory usage spikes
labels:
    - C-bug
    - S-needs-triage
    - S-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.009909Z
info:
    author: wojtek0x
    created_at: 2025-12-01T15:54:36Z
    updated_at: 2026-01-16T02:16:49Z
---

### Describe the bug

We run `reth` on AWS using `r8gd.12xlarge` instance type with 384 GB of RAM. Nodes are handling `eth_call`s at 100-500 RPC/s rate. Typically the memory usage is at ~6%. Several times per day the memory usage starts climbing up and after a few minutes it reaches 50%-100% of the node memory capacity, often resulting with the reth process getting killed. 

A few observations:
* we only see this issue on nodes under RPC load
* it is affecting multiple nodes at the same time
* it is happening for both `reth` on mainnet and `op-reth` on Base mainnet
* I believe I've seen this pattern on 1.8.*, and maybe earlier, but the issue became more severe for us recently

### Steps to reproduce

I reproduced this on a non-production node in order to collect more diagnostic data. I used `i8ge.2xlarge` EC2 instance type with 64 GB of RAM.

I've built the `reth` binary following instructions from https://reth.rs/run/faq/profiling/:
```
git checkout v1.9.3
RUSTFLAGS="-C target-cpu=native" cargo build --features jemalloc-prof --profile profiling
```

I run the node with the following command:
```
_RJEM_MALLOC_CONF=prof:true,lg_prof_interval:32,lg_prof_sample:23,prof_prefix:/mnt/data/profiles2/jemalloc-reth ~/reth/target/profiling/reth node \
--full \
--datadir /mnt/data/reth \
--authrpc.addr 127.0.0.1 \
--authrpc.port 8551 \
--http \
--http.addr 0.0.0.0 \
--http.port 8545 \
--http.api debug,eth,net,trace,txpool,web3,rpc,admin \
--chain mainnet \
--metrics 0.0.0.0:9001 \
--ws \
--ws.addr 0.0.0.0 \
--ws.port 3334 \
--ws.api debug,eth,net,trace,txpool,web3,rpc \
--rpc.gascap 18446744073709551615 \
--log.file.name /mnt/data/profiles2/reth.log
```

I've mirrored production traffic to the test node at fixed rate of 100 RPC/s. The node crashed after ~2h. Screenshots from reth dashboard:

<img width="1412" height="641" alt="Image" src="https://github.com/user-attachments/assets/40e187c3-d944-4295-9a57-831f0eafaff0" />

<img width="1642" height="824" alt="Image" src="https://github.com/user-attachments/assets/2753ddad-c4df-4d6b-a422-5b47e021c6ec" />

Memory profile:
```
Total: 66532734378 B
66532734378 100.0% 100.0% 66532734378 100.0% prof_backtrace_impl
       0   0.0% 100.0% 37921949   0.1% ::add_transactions::{{closure}} (inline)
       0   0.0% 100.0% 53175306636  79.9% ::alloc (inline)
       0   0.0% 100.0% 16810005   0.0% ::alloc_zeroed (inline)
       0   0.0% 100.0% 53183695248  79.9% ::allocate (inline)
       0   0.0% 100.0% 16810005   0.0% ::allocate_zeroed (inline)
       0   0.0% 100.0% 25165920   0.0% ::basic (inline)
       0   0.0% 100.0% 25165920   0.0% ::basic_account
       0   0.0% 100.0% 25165920   0.0% ::basic_account (inline)
       0   0.0% 100.0% 25165920   0.0% ::basic_ref (inline)
(jeprof)
```

### Node logs

```text

```

### Platform(s)

Linux (ARM)

### Container Type

Kubernetes

### What version/commit are you on?

Reth Version: 1.9.3
Commit SHA: 27a8c0f5a6dfb27dea84c5751776ecabdd069646
Build Timestamp: 2025-11-18T15:02:53.152466974Z
Build Features: asm_keccak,jemalloc,min_debug_logs,otlp
Build Profile: maxperf

### What database version are you on?

Current database version: 2
Local database version: 2

### Which chain / network are you on?

mainnet

### What type of node are you running?

Full via --full flag

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
