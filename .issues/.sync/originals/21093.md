---
title: Reth does not commit blocks to the DB
labels:
    - C-bug
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.019622Z
info:
    author: bakhtin
    created_at: 2026-01-15T11:03:41Z
    updated_at: 2026-01-16T12:45:17Z
---

### Describe the bug

Reth does not commit blocks to the DB on disk. Newly synced blocks stay in memory forever until the manual reth restart. On graceful process restart, reth flushes all blocks in memory to the disk and continues to accumulate new blocks in memory.

I inspected the DB file on disk and found that the last saved block is always the latest one reth flushes before the restart.

I left reth running over the night to see what happens when the machine runs out of memory but after some threshold reth finally flushed all the blocks to the disk without a restart. After that reth no longer exhibited the behavior I described above. It now always flushes each block to the disk.

The described behavior happened on two different identically configured VMs

Note that I run the node with `--engine.persistence-threshold=0 --engine.memory-block-buffer-target=0`. I have not tried running without these options. But I tried tuning `engine.persistence-threshold` setting it to 1 and 2 without any effect. I also tried setting `engine.memory-block-buffer-target=10` without any effect.

When reth accumulates blocks in memory, the reported `reth_sync_checkpoint` metric stays flat. The jumps 17:00 and 23:00 is when I manually tried restarting reth.

At 23:18 the issue is self-resolved. Note the memory consumption graph.

<img width="2204" height="1214" alt="Image" src="https://github.com/user-attachments/assets/04bd4e43-59e1-4cea-a106-f2e39a8f7ef1" />

<img width="2189" height="1198" alt="Image" src="https://github.com/user-attachments/assets/00d74202-9d25-41a1-9007-378ff2c25d8e" />

Not sure if important, but the time drift on one of the nodes is ~0.1s. That node took longer to auto-recover blocks flushing. On the other node the time drift is ~0.007s but it was larger when reth was misbehaving.

### Steps to reproduce

Run the node as described below. Probably, `--engine.persistence-threshold=0 --engine.memory-block-buffer-target=0` are the culprit.

### Node logs

```text
Jan 14 19:12:44 host-1 reth[23322]: {"timestamp":"2026-01-14T19:12:44.278858Z","level":"INFO","fields":{"message":"Canonical chain committed","number":24234955,"hash":"0x8cfe26823bdc4518a5273ece8418986ffe3a88cac3e8c2dae24ce744d7b05791","elapsed":"47.543451ms"}}
Jan 14 19:12:46 host-1 reth[23322]: {"timestamp":"2026-01-14T19:12:46.529705Z","level":"INFO","fields":{"message":"New payload job created","id":"0x679aaa11c2fcea19","parent":"0x8cfe26823bdc4518a5273ece8418986ffe3a88cac3e8c2dae24ce744d7b05791"}}
Jan 14 19:12:46 host-1 reth[23322]: {"timestamp":"2026-01-14T19:12:46.587145Z","level":"INFO","fields":{"message":"Status","connected_peers":12,"latest_block":"24234955"}}
Jan 14 19:12:49 host-1 reth[23322]: {"timestamp":"2026-01-14T19:12:49.122926Z","level":"INFO","fields":{"message":"Received block from consensus engine","number":24234956,"hash":"0xb108e9fd8240ee7b8eec108d4de7a4b9d98c45b9a3cb60ae3866743caecb6e57"}}
Jan 14 19:12:49 host-1 reth[23322]: {"timestamp":"2026-01-14T19:12:49.455580Z","level":"INFO","fields":{"message":"State root task finished","state_root":"0xd38df3a80d6b6f7b2b05c30e9937698c105fe9973a89444c6fcd983635f97e7c","elapsed":"32.055839ms"}}
Jan 14 19:12:49 host-1 reth[23322]: {"timestamp":"2026-01-14T19:12:49.455969Z","level":"INFO","fields":{"message":"Block added to canonical chain","number":24234956,"hash":"0xb108e9fd8240ee7b8eec108d4de7a4b9d98c45b9a3cb60ae3866743caecb6e57","peers":12,"txs":351,"gas_used":"34.75Mgas","gas_throughput":"104.66Mgas/second","gas_limit":"59.94Mgas","full":"58.0%","base_fee":"0.13Gwei","blobs":5,"excess_blobs":1415,"elapsed":"331.988834ms"}}
Jan 14 19:12:49 host-1 reth[23322]: {"timestamp":"2026-01-14T19:12:49.637062Z","level":"INFO","fields":{"message":"Canonical chain committed","number":24234956,"hash":"0xb108e9fd8240ee7b8eec108d4de7a4b9d98c45b9a3cb60ae3866743caecb6e57","elapsed":"1.199481ms"}}
Jan 14 19:12:49 host-1 reth[23322]: {"timestamp":"2026-01-14T19:12:49.638449Z","level":"INFO","fields":{"message":"New payload job created","id":"0x83f636ef2463d0bb","parent":"0xb108e9fd8240ee7b8eec108d4de7a4b9d98c45b9a3cb60ae3866743caecb6e57"}}
```

More logs running the node with `RUST_LOG=info,engine::tree=debug`
[reth.log.tar.gz](https://github.com/user-attachments/files/24649549/reth.log.tar.gz)

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

```
Reth Version: 1.9.3
Commit SHA: 27a8c0f5a6dfb27dea84c5751776ecabdd069646
Build Timestamp: 2025-11-27T12:50:20.845930924Z
Build Features: asm_keccak,jemalloc,min_debug_logs,otlp
Build Profile: release
```

### What database version are you on?

```
reth db --datadir /var/lib/persistent/reth/ version
2026-01-15T10:25:19.043659Z  INFO Initialized tracing, debug log directory: /root/.cache/reth/logs/mainnet
Current database version: 2
Local database version: 2
```

### Which chain / network are you on?

mainnet

### What type of node are you running?

Full via --full flag

### What prune config do you use, if any?

```
reth node --full --datadir /var/lib/persistent/reth --authrpc.addr 127.0.0.1 --authrpc.jwtsecret /run/eth-jwt/jwt --authrpc.port 8551 --http --http.addr 127.0.0.1 --http.port 8545 --http.api eth,net,web3,trace,rpc,debug,txpool --ws --ws.addr 127.0.0.1 --ws.port 8546 --ws.api eth,net,trace,web3,rpc,debug,txpool --log.stdout.format json --log.file.max-files 0 --metrics 127.0.0.1:9001 --engine.persistence-threshold=0 --engine.memory-block-buffer-target=0 --ipcpath=/run/reth/reth.ipc
```

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
