---
title: Additional indexes for block explorers
labels:
    - A-db
    - M-prevent-stale
    - S-needs-design
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.983688Z
info:
    author: paulmillr
    created_at: 2025-03-30T06:36:18Z
    updated_at: 2025-04-24T10:08:21Z
---

I've built [esplr](https://github.com/paulmillr/esplr) - private eth explorer. It's getting more popular, but still can't be ran on reth. The functionality of fetching account data from a node can be useful to wallets and other apps. Especially if we make it cross-chain.

### What's needed

- A way to get a full list of historical token transfers for an account.
- A way to get current token balances for an account

### How it currently works

While reth has ots_ namespace, and #14925 enabled proper trace_filter limits, apparently there are some indexes present in Erigon which Reth doesn't have. It makes Reth less useful for explorers which use trace_filter. Initial thoughts on how this can be solved:

a. Add additional indexes. Perhaps target next major version if that requires DB change / resync.
b. Implement additional APIs to get list of token balances and token tx histories.
c. Utilize existing ots_ namespace to replace trace_filter calls
d. Something else? I don't know Reth architecture, the team prob knows better

Esplr uses logic from [micro-eth-signer archive.ts](https://github.com/paulmillr/micro-eth-signer/blob/main/src/net/archive.ts) to implement exploring:

```js
const res = await this.call('trace_filter', {
fromBlock: undefined,
toBlock: undefined,
toAddress: [address],
fromAddress: [address],
});

// tokenTransfers
this.ethLogs(ERC_TRANSFER.topics({ from: address, to: null, value: null }), opts), // From
this.ethLogs(ERC_TRANSFER.topics({ from: null, to: address, value: null }), opts), // To

// erc1155Transfers
this.ethLogs(
  ERC1155_SINGLE.topics({ operator: null, from: address, to: null, id: null, value: null }),
  opts
),
this.ethLogs(
  ERC1155_SINGLE.topics({ operator: null, from: null, to: address, id: null, value: null }),
  opts
),
// Batch
this.ethLogs(
  ERC1155_BATCH.topics({ operator: null, from: address, to: null, ids: null, values: null }),
  opts
),
this.ethLogs(
  ERC1155_BATCH.topics({ operator: null, from: null, to: address, ids: null, values: null }),
  opts
),
```


### Steps to reproduce

Try using esplr (purely client-side web app) with reth

https://paulmillr.com/apps/esplr/


### Code of Conduct

- [x] I agree to follow the Code of Conduct
