# JSON-RPC

You can interact with Reth over JSON-RPC. Reth supports all standard Ethereum JSON-RPC API methods.

JSON-RPC is provided on multiple transports. Reth supports HTTP, WebSocket and IPC (both UNIX sockets and Windows named pipes). Transports must be enabled through command-line flags.

The JSON-RPC APIs are grouped into namespaces, depending on their purpose. All method names are composed of their namespace and their name, separated by an underscore.

Each namespace must be explicitly enabled.

## Namespaces

The methods are grouped into namespaces, which are listed below:

| Namespace               | Description                                                                                            | Sensitive |
|-------------------------|--------------------------------------------------------------------------------------------------------|-----------|
| [`eth`](./eth.md)       | The `eth` API allows you to interact with Ethereum.                                                    | Maybe     |
| [`web3`](./web3.md)     | The `web3` API provides utility functions for the web3 client.                                         | No        |
| [`net`](./net.md)       | The `net` API provides access to network information of the node.                                      | No        |
| [`txpool`](./txpool.md) | The `txpool` API allows you to inspect the transaction pool.                                           | No        |
| [`debug`](./debug.md)   | The `debug` API provides several methods to inspect the Ethereum state, including Geth-style traces.   | No        |
| [`trace`](./trace.md)   | The `trace` API provides several methods to inspect the Ethereum state, including Parity-style traces. | No        |
| [`admin`](./admin.md)   | The `admin` API allows you to configure your node.                                                     | **Yes**   |
| [`rpc`](./rpc.md)       | The `rpc` API provides information about the RPC server and its modules.                               | No        |

Note that some APIs are sensitive, since they can be used to configure your node (`admin`), or access accounts stored on the node (`eth`).

Generally, it is advisable to not expose any JSONRPC namespace publicly, unless you know what you are doing.


## Transports

Reth supports HTTP, WebSockets and IPC.

### HTTP

Using the HTTP transport, clients send a request to the server and immediately get a response back. The connection is closed after the response for a given request is sent.

Because HTTP is unidirectional, subscriptions are not supported.

To start an HTTP server, pass `--http` to `reth node`:

```bash
reth node --http
```

The default port is `8545`, and the default listen address is localhost.

You can configure the listen address and port using `--http.addr` and `--http.port` respectively:

```bash
reth node --http --http.addr 127.0.0.1 --http.port 12345
```

To enable JSON-RPC namespaces on the HTTP server, pass each namespace separated by a comma to `--http.api`:

```bash
reth node --http --http.api eth,net,trace
```

You can also restrict who can access the HTTP server by specifying a domain for Cross-Origin requests. This is important, since any application local to your node will be able to access the RPC server:

```bash
reth node --http --http.corsdomain https://mycoolapp.rs
```

Alternatively, if you want to allow any domain, you can pass `*`:

```bash
reth node --http --http.corsdomain "*"
```

### WebSockets

WebSockets is a bidirectional transport protocol. Most modern browsers support WebSockets.

A WebSocket connection is maintained until it is explicitly terminated by either the client or the node.

Because WebSockets are bidirectional, nodes can push events to clients, which enables clients to subscribe to specific events, such as new transactions in the transaction pool, and new logs for smart contracts.

The configuration of the WebSocket server follows the same pattern as the HTTP server:

- Enable it using `--ws`
- Configure the server address by passing `--ws.addr` and `--ws.port` (default `8546`)
- Configure cross-origin requests using `--ws.origins`
- Enable APIs using `--ws.api`

### IPC

IPC is a simpler transport protocol for use in local environments where the node and the client exist on the same machine.

The IPC transport is enabled by default and has access to all namespaces, unless explicitly disabled with `--ipcdisable`.

Reth creates a UNIX socket on Linux and macOS at `/tmp/reth.ipc`. On Windows, IPC is provided using named pipes at `\\.\pipe\reth.ipc`.

You can configure the IPC path using `--ipcpath`.

## Interacting with the RPC

One can easily interact with these APIs just like they would with any Ethereum client.

You can use `curl`, a programming language with a low-level library, or a tool like Foundry to interact with the chain at the exposed HTTP or WS port.

As a reminder, you need to run the command below to enable all of these APIs using an HTTP transport:

```bash
RUST_LOG=info reth node --http --http.api "admin,debug,eth,net,trace,txpool,web3,rpc"
```

This allows you to then call:

```bash
cast block-number
cast rpc admin_nodeInfo
cast rpc debug_traceTransaction
cast rpc trace_replayBlockTransactions
```
