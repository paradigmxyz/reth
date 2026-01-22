---
title: Add eth_getAddressesInBlock support
labels:
    - A-rpc
    - C-enhancement
    - D-good-first-issue
assignees:
    - 0xChaddB
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.004Z
info:
    author: mattsse
    created_at: 2025-11-09T12:43:14Z
    updated_at: 2025-11-15T09:16:21Z
---

### Describe the feature

see also: https://github.com/paradigmxyz/reth/issues/4054#issuecomment-1669562345

and https://github.com/ethereum/execution-apis/pull/456/files

## TODO
* add ethapi trait fn and impl 


https://github.com/paradigmxyz/reth/blob/43e5cc7989fb4ef8c303ddf639c7a082a51deab1/crates/rpc/rpc-eth-api/src/core.rs#L56-L56

we still need to figure out in which trait fn we want to put this, like
https://github.com/paradigmxyz/reth/blob/43e5cc7989fb4ef8c303ddf639c7a082a51deab1/crates/rpc/rpc-eth-api/src/helpers/trace.rs#L27-L27

or perhaps a new one that combines some of

https://github.com/paradigmxyz/reth/blob/43e5cc7989fb4ef8c303ddf639c7a082a51deab1/crates/rpc/rpc-eth-api/src/helpers/mod.rs#L46-L46

but what we need as implementation is more or less similar to

https://github.com/paradigmxyz/reth/blob/43e5cc7989fb4ef8c303ddf639c7a082a51deab1/crates/rpc/rpc/src/debug.rs#L88-L97

or 

https://github.com/paradigmxyz/reth/blob/43e5cc7989fb4ef8c303ddf639c7a082a51deab1/crates/rpc/rpc/src/debug.rs#L645-L657

where we need to
1. fetch the block
2. execute the entire block
3. check all accessed accounts, including logs and withdrawals in the block
4. return a unique set of addresses 



### Additional context

_No response_
