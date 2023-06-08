# `trace` Namespace

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
- [`trace_callMany`](#trace_callMany)
- [`trace_rawTransaction`](#trace_rawTransaction)
- [`trace_replayBlockTransactions`](#trace_replayBlockTransactions)
- [`trace_replayTransaction`](#trace_replayTransaction)

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