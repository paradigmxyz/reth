---
title: Improve replay/trace evm rpc error message
labels:
    - A-rpc
    - C-enhancement
    - D-good-first-issue
assignees:
    - reject-i
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.013182Z
info:
    author: mattsse
    created_at: 2025-12-11T12:41:55Z
    updated_at: 2025-12-11T12:43:20Z
---

### Describe the feature

currently we might fail during block replay or multi tx execution (such as simBundle)

in case this fails, ideally we attach the the index to the error message, such as:

```
Revm error: transaction execution failed (index 0): out of gas: gas exhausted: 50000000 
```

for this we need a better way to map the evm error:

https://github.com/paradigmxyz/reth/blob/2e567d66587fdafa538e5dbc330a860e61e0067a/crates/rpc/rpc-eth-types/src/error/api.rs#L89-L92

here for example:

https://github.com/paradigmxyz/reth/blob/2e567d66587fdafa538e5dbc330a860e61e0067a/crates/rpc/rpc/src/eth/sim_bundle.rs#L281-L283

or 

https://github.com/paradigmxyz/reth/blob/2e567d66587fdafa538e5dbc330a860e61e0067a/crates/rpc/rpc/src/debug.rs#L130-L135


error mapping happens here:

https://github.com/paradigmxyz/reth/blob/2e567d66587fdafa538e5dbc330a860e61e0067a/crates/rpc/rpc-eth-types/src/error/mod.rs#L493-L497

## TODO:

* introduce a helper for `Indexed<EvmErr`
* map errors with the tx index




### Additional context

_No response_
