---
title: eth_getProof historical flag issues
labels:
    - A-db
    - A-op-reth
    - A-rpc
    - C-bug
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
parent: 18070
synced_at: 2026-01-21T11:32:15.983018Z
info:
    author: kunal365roy
    created_at: 2025-03-19T03:03:11Z
    updated_at: 2025-12-10T14:25:13Z
---

### Describe the bug

Setting the --rpc.eth-proof-window=100000 and querying an old eth_getProof results in some db storage failures

### Steps to reproduce

curl -X POST \
  -H "Content-Type: application/json" \
  --data '{"method":"eth_getProof","params":["0x7d4e742018fb52e48b08be73d041c18b21de6fb5",["0x000000000000000000000000000000000000000000000000000000000000000b","0x2b5f51b8d5fd8c86e620d943a5d9f2b687dfba09c981eed9fde28886282d3a4f","0xfb44bcdd0398172ec04229ecdf2731caab3b9195751a90735b5969e03b3bac03"],"0x15037fe"],"id":29658668,"jsonrpc":"2.0"}' \
  http://localhost:8545/

ewaulra in

{"jsonrpc":"2.0","id":29658668,"error":{"code":-32603,"message":"failed to read a value from a database table: read transaction has been timed out (-96000)"}}

### Node logs

```text
Can add as needed
```

### Platform(s)

Linux (x86)

### Container Type

Not running in a container

### What version/commit are you on?

eth/v1.2.0-1e965ca/x86_640-unknown-linux-gnu

### What database version are you on?

db version fails here

2025-03-18T22:43:59.990088Z  INFO Initialized tracing, debug log directory: /root/.cache/reth/logs/mainnet
Error: Datadir does not exist: "/root/.local/share/reth/mainnet"

Location:
    /root/reth/crates/cli/commands/src/db/mod.rs:74:9

### Which chain / network are you on?

ETH_MAINNET

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

xecStart=/usr/local/bin/reth node \   --ws --ws.port=8545 --ws.addr=0.0.0.0   --ws.origins="*" \   --http --http.port=8545 --http.addr=0.0.0.0 \   --http.corsdomain="*" \   --rpc.gascap 5000000000 \   --rpc.max-subscriptions-per-connection 10000 \   --rpc.max-logs-per-response 0 \   --authrpc.addr 127.0.0.1 \   --authrpc.jwtsecret=/data/jwt.hex \   --rpc.max-connections 429496729 \   --authrpc.port=8551 \   --datadir=/data/reth-data/ \   --http.api=eth,trace,net,web3,debug --ws.api=eth,trace,net,web3,debug \   --metrics 0.0.0.0:6060 \   --rpc.eth-proof-window=100000

### Code of Conduct

- [x] I agree to follow the Code of Conduct
