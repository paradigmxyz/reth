# `trace` Namespace

<!-- TODO: We should probably document the format of the traces themselves, OE does not do that !-->

The `trace` API provides several methods to inspect the Ethereum state, including Parity-style traces.

A similar module exists (with other debug functions) with Geth-style traces ([`debug`](./debug.md)).

The `trace` API gives deeper insight into transaction processing.

There are two types of methods in this API:

- **Ad-hoc tracing APIs** for performing diagnostics on calls or transactions (historical or hypothetical).
- **Transaction-trace filtering APIs** for getting full externality traces on any transaction executed by reth.

## Ad-hoc tracing APIs

Ad-hoc tracing APIs allow you to perform diagnostics on calls or transactions (historical or hypothetical), including:

- Transaction traces (`trace`)
- VM traces (`vmTrace`)
- State difference traces (`stateDiff`)

The ad-hoc tracing APIs are:

- [`trace_call`](#trace_call)
- [`trace_callMany`](#trace_callmany)
- [`trace_rawTransaction`](#trace_rawtransaction)
- [`trace_replayBlockTransactions`](#trace_replayblocktransactions)
- [`trace_replayTransaction`](#trace_replaytransaction)

## Transaction-trace filtering APIs

Transaction trace filtering APIs are similar to log filtering APIs in the `eth` namespace, except these allow you to search and filter based only upon address information.

Information returned includes the execution of all contract creations, destructions, and calls, together with their input data, output data, gas usage, transfer amounts and success statuses.

The transaction trace filtering APIs are:

- [`trace_block`](#trace_block)
- [`trace_filter`](#trace_filter)
- [`trace_get`](#trace_get)
- [`trace_transaction`](#trace_transaction)

## `trace_call`

Executes the given call and returns a number of possible traces for it.

The first parameter is a transaction object where the `from` field is optional and the `nonce` field is ommitted.

The second parameter is an array of one or more trace types (`vmTrace`, `trace`, `stateDiff`).

The third and optional parameter is a block number, block hash, or a block tag (`latest`, `finalized`, `safe`, `earliest`, `pending`).

| Client | Method invocation                                         |
|--------|-----------------------------------------------------------|
| RPC    | `{"method": "trace_call", "params": [tx, type[], block]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"trace_call","params":[{},["trace"]}
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": {
        "output": "0x",
        "stateDiff": null,
        "trace": [{
            "action": { ... },
            "result": {
                "gasUsed": "0x0",
                "output": "0x"
            },
            "subtraces": 0,
            "traceAddress": [],
            "type": "call"
        }],
        "vmTrace": null
    }
}
```

## `trace_callMany`

Performs multiple call traces on top of the same block, that is, transaction `n` will be executed on top of a pending block with all `n - 1` transaction applied (and traced) first.

The first parameter is a list of call traces, where each call trace is of the form `[tx, type[]]` (see [`trace_call`](#trace_call)).

The second and optional parameter is a block number, block hash, or a block tag (`latest`, `finalized`, `safe`, `earliest`, `pending`).

| Client | Method invocation                                      |
|--------|--------------------------------------------------------|
| RPC    | `{"method": "trace_call", "params": [trace[], block]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"trace_callMany","params":[[[{"from":"0x407d73d8a49eeb85d32cf465507dd71d507100c1","to":"0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b","value":"0x186a0"},["trace"]],[{"from":"0x407d73d8a49eeb85d32cf465507dd71d507100c1","to":"0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b","value":"0x186a0"},["trace"]]],"latest"]}
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": [
        {
            "output": "0x",
            "stateDiff": null,
            "trace": [{
                "action": {
                    "callType": "call",
                    "from": "0x407d73d8a49eeb85d32cf465507dd71d507100c1",
                    "gas": "0x1dcd12f8",
                    "input": "0x",
                    "to": "0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b",
                    "value": "0x186a0"
                },
                "result": {
                    "gasUsed": "0x0",
                    "output": "0x"
                },
                "subtraces": 0,
                "traceAddress": [],
                "type": "call"
            }],
            "vmTrace": null
        },
        {
            "output": "0x",
            "stateDiff": null,
            "trace": [{
                "action": {
                    "callType": "call",
                    "from": "0x407d73d8a49eeb85d32cf465507dd71d507100c1",
                    "gas": "0x1dcd12f8",
                    "input": "0x",
                    "to": "0xa94f5374fce5edbc8e2a8697c15331677e6ebf0b",
                    "value": "0x186a0"
                },
                "result": {
                    "gasUsed": "0x0",
                    "output": "0x"
                },
                "subtraces": 0,
                "traceAddress": [],
                "type": "call"
            }],
            "vmTrace": null
        }
    ]
}
```

## `trace_rawTransaction`

Traces a call to `eth_sendRawTransaction` without making the call, returning the traces.

| Client | Method invocation                                      |
|--------|--------------------------------------------------------|
| RPC    | `{"method": "trace_call", "params": [raw_tx, type[]]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"trace_rawTransaction","params":["0xd46e8dd67c5d32be8d46e8dd67c5d32be8058bb8eb970870f072445675058bb8eb970870f072445675",["trace"]]}
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": {
        "output": "0x",
            "stateDiff": null,
            "trace": [{
            "action": { ... },
            "result": {
                "gasUsed": "0x0",
                "output": "0x"
            },
            "subtraces": 0,
            "traceAddress": [],
            "type": "call"
        }],
            "vmTrace": null
    }
}
```

## `trace_replayBlockTransactions`

Replays all transactions in a block returning the requested traces for each transaction.

| Client | Method invocation                                                        |
|--------|--------------------------------------------------------------------------|
| RPC    | `{"method": "trace_replayBlockTransactions", "params": [block, type[]]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"trace_replayBlockTransactions","params":["0x2ed119",["trace"]]}
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": [
        {
            "output": "0x",
            "stateDiff": null,
            "trace": [{
                "action": { ... },
                "result": {
                    "gasUsed": "0x0",
                    "output": "0x"
                },
                "subtraces": 0,
                "traceAddress": [],
                "type": "call"
            }],
            "transactionHash": "0x...",
            "vmTrace": null
        },
        { ... }
    ]
}
```

## `trace_replayTransaction`

Replays a transaction, returning the traces.

| Client | Method invocation                                                    |
|--------|----------------------------------------------------------------------|
| RPC    | `{"method": "trace_replayTransaction", "params": [tx_hash, type[]]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"trace_replayTransaction","params":["0x02d4a872e096445e80d05276ee756cefef7f3b376bcec14246469c0cd97dad8f",["trace"]]}
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": {
        "output": "0x",
        "stateDiff": null,
        "trace": [{
            "action": { ... },
            "result": {
                "gasUsed": "0x0",
                "output": "0x"
            },
            "subtraces": 0,
            "traceAddress": [],
            "type": "call"
        }],
        "vmTrace": null
    }
}
```

## `trace_block`

Returns traces created at given block.

| Client | Method invocation                              |
|--------|------------------------------------------------|
| RPC    | `{"method": "trace_block", "params": [block]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"trace_block","params":["0x2ed119"]}
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": [
        {
            "action": {
                "callType": "call",
                "from": "0xaa7b131dc60b80d3cf5e59b5a21a666aa039c951",
                "gas": "0x0",
                "input": "0x",
                "to": "0xd40aba8166a212d6892125f079c33e6f5ca19814",
                "value": "0x4768d7effc3fbe"
            },
            "blockHash": "0x7eb25504e4c202cf3d62fd585d3e238f592c780cca82dacb2ed3cb5b38883add",
            "blockNumber": 3068185,
            "result": {
                "gasUsed": "0x0",
                "output": "0x"
            },
            "subtraces": 0,
            "traceAddress": [],
            "transactionHash": "0x07da28d752aba3b9dd7060005e554719c6205c8a3aea358599fc9b245c52f1f6",
            "transactionPosition": 0,
            "type": "call"
        },
        ...
    ]
}
```

## `trace_filter`

Returns traces matching given filter.

Filters are objects with the following properties:

- `fromBlock`: Returns traces from the given block (a number, hash, or a tag like `latest`).
- `toBlock`: Returns traces to the given block.
- `fromAddress`: Sent from these addresses
- `toAddress`: Sent to these addresses
- `after`: The offset trace number
- `count`: The number of traces to display in a batch

All properties are optional.

| Client | Method invocation                                |
|--------|--------------------------------------------------|
| RPC    | `{"method": "trace_filter", "params": [filter]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"trace_filter","params":[{"fromBlock":"0x2ed0c4","toBlock":"0x2ed128","toAddress":["0x8bbB73BCB5d553B5A556358d27625323Fd781D37"],"after":1000,"count":100}]}
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": [
        {
            "action": {
                "callType": "call",
                "from": "0x32be343b94f860124dc4fee278fdcbd38c102d88",
                "gas": "0x4c40d",
                "input": "0x",
                "to": "0x8bbb73bcb5d553b5a556358d27625323fd781d37",
                "value": "0x3f0650ec47fd240000"
            },
            "blockHash": "0x86df301bcdd8248d982dbf039f09faf792684e1aeee99d5b58b77d620008b80f",
            "blockNumber": 3068183,
            "result": {
                "gasUsed": "0x0",
                "output": "0x"
            },
            "subtraces": 0,
            "traceAddress": [],
            "transactionHash": "0x3321a7708b1083130bd78da0d62ead9f6683033231617c9d268e2c7e3fa6c104",
            "transactionPosition": 3,
            "type": "call"
        },
        ...
    ]
}
```

## `trace_get`

Returns trace at given position.

| Client | Method invocation                                        |
|--------|----------------------------------------------------------|
| RPC    | `{"method": "trace_get", "params": [tx_hash,indices[]]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"trace_get","params":["0x17104ac9d3312d8c136b7f44d4b8b47852618065ebfa534bd2d3b5ef218ca1f3",["0x0"]]}
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": {
        "action": {
            "callType": "call",
            "from": "0x1c39ba39e4735cb65978d4db400ddd70a72dc750",
            "gas": "0x13e99",
            "input": "0x16c72721",
            "to": "0x2bd2326c993dfaef84f696526064ff22eba5b362",
            "value": "0x0"
        },
        "blockHash": "0x7eb25504e4c202cf3d62fd585d3e238f592c780cca82dacb2ed3cb5b38883add",
            "blockNumber": 3068185,
            "result": {
            "gasUsed": "0x183",
            "output": "0x0000000000000000000000000000000000000000000000000000000000000001"
        },
        "subtraces": 0,
            "traceAddress": [
            0
        ],
        "transactionHash": "0x17104ac9d3312d8c136b7f44d4b8b47852618065ebfa534bd2d3b5ef218ca1f3",
        "transactionPosition": 2,
        "type": "call"
    }
}
```

## `trace_transaction`

Returns all traces of given transaction

| Client | Method invocation                                      |
|--------|--------------------------------------------------------|
| RPC    | `{"method": "trace_transaction", "params": [tx_hash]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"trace_transaction","params":["0x17104ac9d3312d8c136b7f44d4b8b47852618065ebfa534bd2d3b5ef218ca1f3"]}
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": [
        {
            "action": {
                "callType": "call",
                "from": "0x1c39ba39e4735cb65978d4db400ddd70a72dc750",
                "gas": "0x13e99",
                "input": "0x16c72721",
                "to": "0x2bd2326c993dfaef84f696526064ff22eba5b362",
                "value": "0x0"
            },
            "blockHash": "0x7eb25504e4c202cf3d62fd585d3e238f592c780cca82dacb2ed3cb5b38883add",
            "blockNumber": 3068185,
            "result": {
                "gasUsed": "0x183",
                "output": "0x0000000000000000000000000000000000000000000000000000000000000001"
            },
            "subtraces": 0,
            "traceAddress": [
                0
            ],
            "transactionHash": "0x17104ac9d3312d8c136b7f44d4b8b47852618065ebfa534bd2d3b5ef218ca1f3",
            "transactionPosition": 2,
            "type": "call"
        },
        ...
    ]
}
```