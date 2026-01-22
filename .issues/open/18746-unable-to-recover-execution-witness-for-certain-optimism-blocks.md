---
title: Unable to recover execution witness for certain Optimism blocks
labels:
    - A-op-reth
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.999596Z
info:
    author: mariaKt
    created_at: 2025-09-26T23:03:14Z
    updated_at: 2025-12-10T15:22:27Z
---

### Describe the bug

Typically, we are able to access the execution witness for the blocks of the optimism mainnet using `debug_executionWitness` on a local op-reth node. However, intermittently (i.e., for some blocks), the attempt to get the witness fails, with the following error: 
```
{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"trie witness error: error in storage trie for address 0xdd41fc54fc5d4b0e7a0f2d2bbd0e8e1485f77deeb29c8ef9b392af50698f04e1: Other(Database(Read(DatabaseErrorInfo { message: \"read transaction has been timed out\", code: -96000 })))"}}
```

We have noticed that the few times this has happened, the address `0xdd41fc54fc5d4b0e7a0f2d2bbd0e8e1485f77deeb29c8ef9b392af50698f04e1` is always the same.

We do not believe this is due to the blocks having been pruned at the time, as we tested immediately and were able to get the witnesses for the blocks immediately before and after.

### Steps to reproduce

1. Run an op-reth node
2. Run `curl -X POST op-reth-node:port  -H "Content-Type: application/json"  --data '{"jsonrpc":"2.0","id":1,"method":"debug_executionWitness","params":["blockNumber"]}'` for the blocks of the optimism mainnet.
3. Unfotunately, the error above is intermittent, so it might take a while to appear. The latest block for which we noticed it was 0x87177b2.

### Node logs

```text

```

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

```
./op-reth-1_7_0 --version
reth-optimism-cli Version: 1.7.0
Commit SHA: 9d56da53ec0ad60e229456a0c70b338501d923a5
Build Timestamp: 2025-09-08T14:46:48.633347953Z
Build Features: asm_keccak,jemalloc
Build Profile: maxperf
```

### What database version are you on?

```
./op-reth-1_7_0 db version
2025-09-26T22:39:10.747400Z  INFO Initialized tracing, debug log directory: /root/.cache/reth/logs/optimism
Error: Datadir does not exist: "/root/.local/share/reth/optimism"

Location:
    /home/runner/work/reth/reth/crates/cli/commands/src/db/mod.rs:79:9
```

### Which chain / network are you on?

op-mainnet

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

_No response_

### Code of Conduct

- [x] I agree to follow the Code of Conduct
