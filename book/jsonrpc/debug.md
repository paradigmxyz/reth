# `debug` Namespace

The `debug` API provides several methods to inspect the Ethereum state, including Geth-style traces.

## `debug_getRawHeader`

Returns an RLP-encoded header.

| Client | Method invocation                                     |
|--------|-------------------------------------------------------|
| RPC    | `{"method": "debug_getRawHeader", "params": [block]}` |

## `debug_getRawBlock`

Retrieves and returns the RLP encoded block by number, hash or tag.

| Client | Method invocation                                    |
|--------|------------------------------------------------------|
| RPC    | `{"method": "debug_getRawBlock", "params": [block]}` |

## `debug_getRawTransaction`

Returns an EIP-2718 binary-encoded transaction.

| Client | Method invocation                                            |
|--------|--------------------------------------------------------------|
| RPC    | `{"method": "debug_getRawTransaction", "params": [tx_hash]}` |

## `debug_getRawReceipts`

Returns an array of EIP-2718 binary-encoded receipts.

| Client | Method invocation                                       |
|--------|---------------------------------------------------------|
| RPC    | `{"method": "debug_getRawReceipts", "params": [block]}` |

## `debug_getBadBlocks`

Returns an array of recent bad blocks that the client has seen on the network.

| Client | Method invocation                                |
|--------|--------------------------------------------------|
| RPC    | `{"method": "debug_getBadBlocks", "params": []}` |

## `debug_traceChain`

Returns the structured logs created during the execution of EVM between two blocks (excluding start) as a JSON object.

| Client | Method invocation                                                    |
|--------|----------------------------------------------------------------------|
| RPC    | `{"method": "debug_traceChain", "params": [start_block, end_block]}` |

## `debug_traceBlock`

The `debug_traceBlock` method will return a full stack trace of all invoked opcodes of all transaction that were included in this block.

This expects an RLP-encoded block.

> **Note**
> 
> The parent of this block must be present, or it will fail.

| Client | Method invocation                                       |
|--------|---------------------------------------------------------|
| RPC    | `{"method": "debug_traceBlock", "params": [rlp, opts]}` |

## `debug_traceBlockByHash`

Similar to [`debug_traceBlock`](#debug_traceblock), `debug_traceBlockByHash` accepts a block hash and will replay the block that is already present in the database.

| Client | Method invocation                                                    |
|--------|----------------------------------------------------------------------|
| RPC    | `{"method": "debug_traceBlockByHash", "params": [block_hash, opts]}` |

## `debug_traceBlockByNumber`

Similar to [`debug_traceBlockByHash`](#debug_traceblockbyhash), `debug_traceBlockByNumber` accepts a block number and will replay the block that is already present in the database.

| Client | Method invocation                                                        |
|--------|--------------------------------------------------------------------------|
| RPC    | `{"method": "debug_traceBlockByNumber", "params": [block_number, opts]}` |

## `debug_traceTransaction`

The `debug_traceTransaction` debugging method will attempt to run the transaction in the exact same manner as it was executed on the network. It will replay any transaction that may have been executed prior to this one before it will finally attempt to execute the transaction that corresponds to the given hash.

| Client | Method invocation                                           |
|--------|-------------------------------------------------------------|
| RPC    | `{"method": "debug_traceTransaction", "params": [tx_hash, opts]}` |

## `debug_traceCall`

The `debug_traceCall` method lets you run an `eth_call` within the context of the given block execution using the final state of parent block as the base.

The first argument (just as in `eth_call`) is a transaction request.

The block can optionally be specified either by hash or by number as the second argument.

| Client | Method invocation                                                     |
|--------|-----------------------------------------------------------------------|
| RPC    | `{"method": "debug_traceCall", "params": [call, block_number, opts]}` |
