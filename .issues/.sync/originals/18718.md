---
title: eth_simulateV1 incorrectly checks against account value when gas is omitted.
labels:
    - A-rpc
    - C-bug
assignees:
    - klkvr
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.9992Z
info:
    author: hamdiallam
    created_at: 2025-09-25T20:14:54Z
    updated_at: 2025-09-25T23:42:11Z
---

### Describe the bug

If `gas` is omitted, there's no way to know the gasUsed until execution. Hence it's impossible to do upfront validation relative to the account value. When `gas`, `maxFeePerGas`, `maxPriorityFeePerGas` are specified, a check against value can be made.

We had a code path where `maxFeePerGas` and `maxPriorityFeePerGas` were set without `gas` (unintentionally). Even though this was unintentional, we shouldn't have seen any simulation errors since the max values were reasonable.

After some debugging it seems like [this is the offending snippet](https://github.com/paradigmxyz/reth/blob/bb09b9704d533732fac61cf15694d31c0b232e4c/crates/rpc/rpc-eth-api/src/helpers/call.rs#L157C25-L161C26) when computing the default gas limit. It seems to distribute the rest of the gas available for the txs without gas. When this is then validated against the account balance, it fails as expected.
```
if txs_without_gas_limit > 0 {
  (block_gas_limit - total_specified_gas) / txs_without_gas_limit as u64
} else {
   0
}
```

### Steps to reproduce

eth_simulateV1 with a low balance account but no `gas`. With `maxFeePerGas` and `maxPriorityFeePerGas` set. Here's an example
```
            {
              "data": "0x095ea7b3000000000000000000000000000000000022d473030f116ddee9f6b43ac78ba3ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
              "from": "0xB6754E53Ce15dF43269F59f21C9c235F1f673d67",
              "maxFeePerGas": "0x8142ab",
              "maxPriorityFeePerGas": "0x16e360",
              "nonce": "0x184b",
              "to": "0xbce4705b674fa0d1dc3c1ba38ca5cfd989eae656",
              "type": "0x2",
              "value": "0x0"
            }
```
```
{
    "jsonrpc": "2.0",
    "id": 32,
    "error": {
        "code": -32003,
        "message": "insufficient funds for gas * price + value: have 54534710340691 want 1270681650000000"
    }
}
```

The "want" is orders of magnitude higher. If the `max` values are omitted, things work as expected. If pasting in sufficient `gas`. Things work as expected.

Not a blocker as we can omit max values without any gas. But believe this is a mismatch in behavior w.r.t how transactions are executed.

### Code of Conduct

- [x] I agree to follow the Code of Conduct
