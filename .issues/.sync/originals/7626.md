---
title: '`eth_getBlockReceipts` returning empty after `reth stage drop merkle`'
labels:
    - A-rpc
    - A-static-files
    - C-bug
    - M-prevent-stale
assignees:
    - jenpaff
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.973451Z
info:
    author: ajsutton
    created_at: 2024-04-13T21:50:08Z
    updated_at: 2025-12-31T06:54:25Z
---

### Describe the bug

No receipts returned for block via `eth_getBlockReceipts` `0x559c89487febbe632474f1921aeddaa0fe3778fb7897b7b5760b2ff1db5c1ba3:5682981` on sepolia. The receipts from the block are available when querying receipts individually.

```
$ cast rpc eth_getBlockReceipts 0x559c89487febbe632474f1921aeddaa0fe3778fb7897b7b5760b2ff1db5c1ba3 --rpc-url $SEPOLIA_RETH
[]
$ cast rpc eth_getBlockReceipts 0x559c89487febbe632474f1921aeddaa0fe3778fb7897b7b5760b2ff1db5c1ba3 --rpc-url $SEPOLIA_ALCHEMY
[{"transactionHash":"0xa8047b8da15c1d630f052839ad13b4a824055eee15a84ca6122f615c46f1ee07","blockHash":"0x559c89487febbe632474f1921aeddaa0fe3778fb7897b7b5760b2ff1db5c1ba3","blockNum.....
```

Running reth in docker with config (the commented out lines show the command used to run stage drop merkle):
```
  el:
    user: "1000"
    image: reth:local
    stop_grace_period: 5m
    ports:
      - 30304:30304/tcp
      - 30304:30304/udp
    labels:
      network: sepolia
    command:
      #- "stage"
      #- "drop"
      #- "merkle"
      #- "--datadir=/data"
      #- "--log.file.directory=/data/logs"
      #- "--log.stdout.format=log-fmt"

      - "node"
      - "--chain=sepolia"
      - "--log.file.directory=/data/logs"
      - "--log.stdout.format=log-fmt"
      - "--authrpc.addr=0.0.0.0"
      - "--authrpc.jwtsecret=/cfg/jwt.txt"
      - "--http"
      - "--http.addr=0.0.0.0"
      - "--http.api=eth,net,web3,debug,trace,admin,rpc,txpool"
      - "--datadir=/data"
      - "--metrics=0.0.0.0:8008"
      - "--port=30304"
      - "--discovery.port=30304"
    volumes:
      - "./data/reth:/data"
      - "./cfg:/cfg"
    restart: always
    networks:
      - sepolia
      - metrics
```

### Steps to reproduce

Haven't attempted to reproduce yet, but sequence of events was that I encountered a couple of incorrectly rejected blocks on sepolia because of state root mismatches.  See https://github.com/paradigmxyz/reth/issues/7559#issuecomment-2053598305 
Stopped the node, ran `reth stage drop merkle` then started again. It resumed following the chain but was missing receipts for at least block `0x559c89487febbe632474f1921aeddaa0fe3778fb7897b7b5760b2ff1db5c1ba3:5682981`

### Node logs

```text
https://gist.githubusercontent.com/ajsutton/835ac1b7f4a37a72948bb2ded23ef266/raw/2501feb0fa5dc7c7f394444dc4f6e9484dcc6203/gistfile1.txt
```


### Platform(s)

Linux (x86)

### What version/commit are you on?

0.2.0-beta.5-dev (041e29347)

### What database version are you on?

Current database version: 2
Local database version: 2

### What type of node are you running?

Archive (default)

### What prune config do you use, if any?

_No response_

### If you've built Reth from source, provide the full command you used

docker build . -t reth:local

### Code of Conduct

- [X] I agree to follow the Code of Conduct
