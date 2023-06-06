# JSON-RPC Namespaces

Reth supports all standard Ethereum JSON-RPC API methods. The API methods are grouped into namespaces, which are listed below:
* [admin](./admin.md): Administrative APIs for the node. DO NOT expose these APIs to the public internet.
* [eth](./eth.md): Ethereum APIs for interacting with the Ethereum blockchain.
* [tracing](./tracing.md): APIs for tracing the execution of transactions, compatible with the popular [Parity Tracing module](https://openethereum.github.io/JSONRPC-trace-module).
* [debug](./debug.md): APIs for debugging the node, originally created by Geth.
<!-- TODO: add missing ones -->

One can easily interact with these APIs just like they would with any Ethereum client. You can use curl, a programming language with a low-level library, or a tool like Foundry to interact with the chain at the exposed HTTP or WS port. As a reminder, you need to run the command below to enable all of these apis:

```bash
RUST_LOG=info reth node --http --http.api "admin,debug,eth,net,trace,txpool,web3,rpc"
```

> The IPC transport is also supported with `--ipc`!

This allows you to then call:

```bash
cast block-number
cast rpc admin_nodeInfo
cast rpc debug_traceTransaction
cast rpc trace_replayBlockTransactions
```
