# `net` Namespace

The `net` API provides information about the networking component of the node.

## `net_listening`

Returns a `bool` indicating whether or not the node is listening for network connections.

| Client | Method invocation                           |
|--------|---------------------------------------------|
| RPC    | `{"method": "net_listening", "params": []}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"net_listening","params":[]}
{"jsonrpc":"2.0","id":1,"result":true}
```

## `net_peerCount`

Returns the number of peers connected to the node.

| Client | Method invocation                           |
|--------|---------------------------------------------|
| RPC    | `{"method": "net_peerCount", "params": []}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"net_peerCount","params":[]}
{"jsonrpc":"2.0","id":1,"result":10}
```

## `net_version`

Returns the network ID (e.g. 1 for mainnet)

| Client | Method invocation                         |
|--------|-------------------------------------------|
| RPC    | `{"method": "net_version", "params": []}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"net_version","params":[]}
{"jsonrpc":"2.0","id":1,"result":1}
```