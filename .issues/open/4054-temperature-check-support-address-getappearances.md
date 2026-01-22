---
title: 'Temperature check: support address_getAppearances'
labels:
    - A-rpc
    - C-enhancement
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.970036Z
info:
    author: perama-v
    created_at: 2023-08-04T07:49:59Z
    updated_at: 2025-08-26T09:52:41Z
---

### Describe the feature

This issue is to introduce the concept of "address appearances" and assess appetite for inclusion in reth. 

### Specifications

See also the following specifications:
- What an "appearance" is: https://github.com/ethereum/execution-apis/pull/456
- Namespace and method: https://github.com/ethereum/execution-apis/pull/453
- A related, on-the-fly single block method: https://github.com/ethereum/execution-apis/pull/452

## Motivation

A user (or local application) running reth can start with an address (e.g., `0x30a4...1382`) important to them.
They query reth and get the set of transactions that are relevant to that address. 

```io
>> {"jsonrpc":"2.0","id":1,
    "method":"address_getAppearances",
    "params":["0x30a4639850b3ddeaaca4f06280aa751682f11382"]}
<< {"id":1,"jsonrpc":"2.0","result":[
    {"blockNumber":"0xd63f51","transactionIndex":"0xe6"},
    {"blockNumber":"0xd63f5a","transactionIndex":"0x11b"},
    {"blockNumber":"0xd68154","transactionIndex":"0x6"},
    ...
```
This kicks off the journey, which can now proceed in many ways:
- Inspect the transactions (e.g., trace them, looking for particular things) to get a history of activity.
    - "List/graph all historical ether balances changes for a wallet"
    - "List all contracts interacted with"
- Find the intersection with other addresses (such as a particular contract) to get user interactions with a particular protocol.
    - "List all trades on a particular protocol for some wallet"
    - "List all users who have been involved with a protocol"

As addresses are the foundation of user and protocol activity, the feature is general purpose. 

## Why "in reth" and not separate 

The presence of the `address_getAppearances` natively within reth would mean first class support for user-focused applications. This could lead to:
- More Ethereum users running reth. There is tangible gain for a user: a history of personal activity
- Robust/resilient applications. A new protocol can be deployed and a frontend published that uses `address_getAppearances` to locate relevant transactions. The frontend does not require any node restart / trace filter / external application to function. 

As the method is general purpose it always supports new wallets/protocols that a user is suddenly interested in. 

## Prior work

The UnchainedIndex, accessible via trueblocks-core ([https://github.com/TrueBlocks/trueblocks-core](https://github.com/TrueBlocks/trueblocks-core)) is an example of the utility of the information that `address_getAppearances` would provide. 

The method is equivalent to the following command in `trueblocks-core`:
```console
> chifra list 0x30a4639850b3ddeaaca4f06280aa751682f11382
address	blockNumber	transactionIndex
0x30a4639850b3ddeaaca4f06280aa751682f11382	14040913	230
0x30a4639850b3ddeaaca4f06280aa751682f11382	14040922	283
0x30a4639850b3ddeaaca4f06280aa751682f11382	14057812	6
...
```

## Disk impact

The support for `address_getAppearances` creates additional disk burden. An upper ceiling estimate is 80GB.

The estimate is based on the UnchainedIndex (80GB), which is specially designed for distribution/sharding on IPFS. Such requirements mean that the index adds immutable pieces of data every 12 hours. An address that appears every 12 hours is therefore duplicated. 

There are likely means to significantly reduce the disk footprint of a native reth version of this data.


### Additional context


## What is an "appearance"
Appearances are defined in a way to capture instances when an address arises in a meaningful way in the chain. 

A simple example is the subject of a `CALL` opcode to an EOA. The recipient "appears" in that transaction (they receive ether). Upon tracing, the meaning of the transaction can be determined. 

An "appearance" is defined in the following PR:
- https://github.com/ethereum/execution-apis/pull/456

## `address_getAppearances` method
The method is defined in the following PR:
- https://github.com/ethereum/execution-apis/pull/453

This is a chicken an egg situation: The method is stronger if there is an indication of existence/support in a client and vice versa.

## Temperature check
Seeking feedback on this concept. Do any of the following resonate with you? Please feel free to add thoughts of any kind
1. Is this desirable?
2. Would a PR implementing a flag-based `address_` namespace for `address_getAppearances` be considered?
3. "In favour but, would like to see more concrete disk footprint prototype/estimate"
4. "I can see how this would enable `some_application_idea`"
5. "Please keep it off the node and in companion applications/datasets"
6. `<other>`

Thanks for your time

```[tasklist]
### Checklist
- [ ] `address_` namespace addition with placeholder impl of `address_getAppearances` (#4146)
- [ ] implement a `get_appearances_from_transaction()` method (#4162)
- [ ] implement `eth_getAppearancesInBlock` JSON-RPC method
- [ ] define new `AppearanceHistory` table (https://github.com/paradigmxyz/reth/pull/4182)
- [ ] Populate the index during a stage
- [ ] implement `address_getAppearances` endpoint (`Address -> Vec<(BlockNumber, TxIndex)>`)
- [ ] Design mechanism to add APIs like this w/o introducing large main codebase maintenance overheads (#4325)
```
