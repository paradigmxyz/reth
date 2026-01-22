---
title: Add disable txpool setting to OpEthApi
labels:
    - A-cli
    - A-op-reth
    - C-enhancement
    - D-good-first-issue
assignees:
    - Soubhik-10
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 15759
synced_at: 2026-01-21T11:32:15.98669Z
info:
    author: mattsse
    created_at: 2025-04-23T08:01:39Z
    updated_at: 2025-04-23T08:40:44Z
---

### Describe the feature

ref #15759

we should mirror op-geth 1:1 on eth_sendrawtx:

https://github.com/ethereum-optimism/op-geth/blob/36501a7023fd85f3492a1af6f1474a0113bb83fe/eth/api_backend.go#L297-L299

hence we need a disable_txpool_admisson setting, similar to geth:

https://github.com/ethereum-optimism/op-geth/blob/36501a7023fd85f3492a1af6f1474a0113bb83fe/eth/ethconfig/config.go#L183-L183

on cli, like:

https://github.com/paradigmxyz/reth/blob/211ecb6d91064991d558d9ea4ae0b81c463304d7/crates/optimism/node/src/args.rs#L16-L18

but inverse: 

https://github.com/ethereum-optimism/op-geth/blob/36501a7023fd85f3492a1af6f1474a0113bb83fe/cmd/utils/flags.go#L947-L951

## TODO

* add CLI var mirroring geth
* add setting on OpEthApi https://github.com/paradigmxyz/reth/blob/211ecb6d91064991d558d9ea4ae0b81c463304d7/crates/optimism/rpc/src/eth/mod.rs#L280-L280
* apply check on setup: https://github.com/ethereum-optimism/op-geth/blob/36501a7023fd85f3492a1af6f1474a0113bb83fe/cmd/utils/flags.go#L1906-L1906, deriving disable_txpool based on what flags are set

### Additional context

_No response_
