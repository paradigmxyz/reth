# `admin` Namespace

The `admin` API allows you to configure your node, including adding and removing peers.

> **Note**
> 
> As this namespace can configure your node at runtime, it is generally **not advised** to expose it publicly.

## `admin_addPeer`

Add the given peer to the current peer set of the node.

The method accepts a single argument, the [`enode`][enode] URL of the remote peer to connect to, and returns a `bool` indicating whether the peer was accepted or not.

| Client | Method invocation                              |
|--------|------------------------------------------------|
| RPC    | `{"method": "admin_addPeer", "params": [url]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"admin_addPeer","params":["enode://a979fb575495b8d6db44f750317d0f4622bf4c2aa3365d6af7c284339968eef29b69ad0dce72a4d8db5ebb4968de0e3bec910127f134779fbcb0cb6d3331163c@52.16.188.185:30303"]}
{"jsonrpc":"2.0","id":1,"result":true}
```

## `admin_removePeer`

Disconnects from a peer if the connection exists. Returns a `bool` indicating whether the peer was successfully removed or not.

| Client | Method invocation                                  |
|--------|----------------------------------------------------|
| RPC    | `{"method": "admin_removePeer", "params": [url]}`  |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"admin_removePeer","params":["enode://a979fb575495b8d6db44f750317d0f4622bf4c2aa3365d6af7c284339968eef29b69ad0dce72a4d8db5ebb4968de0e3bec910127f134779fbcb0cb6d3331163c@52.16.188.185:30303"]}
{"jsonrpc":"2.0","id":1,"result":true}
```

## `admin_addTrustedPeer`

Adds the given peer to a list of trusted peers, which allows the peer to always connect, even if there would be no room for it otherwise.

It returns a `bool` indicating whether the peer was added to the list or not.

| Client | Method invocation                                     |
|--------|-------------------------------------------------------|
| RPC    | `{"method": "admin_addTrustedPeer", "params": [url]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"admin_addTrustedPeer","params":["enode://a979fb575495b8d6db44f750317d0f4622bf4c2aa3365d6af7c284339968eef29b69ad0dce72a4d8db5ebb4968de0e3bec910127f134779fbcb0cb6d3331163c@52.16.188.185:30303"]}
{"jsonrpc":"2.0","id":1,"result":true}
```

## `admin_removeTrustedPeer`

Removes a remote node from the trusted peer set, but it does not disconnect it automatically.

Returns true if the peer was successfully removed.

| Client | Method invocation                                        |
|--------|----------------------------------------------------------|
| RPC    | `{"method": "admin_removeTrustedPeer", "params": [url]}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"admin_removeTrustedPeer","params":["enode://a979fb575495b8d6db44f750317d0f4622bf4c2aa3365d6af7c284339968eef29b69ad0dce72a4d8db5ebb4968de0e3bec910127f134779fbcb0cb6d3331163c@52.16.188.185:30303"]}
{"jsonrpc":"2.0","id":1,"result":true}
```

## `admin_nodeInfo`

Returns all information known about the running node.

These include general information about the node itself, as well as what protocols it participates in, its IP and ports.

| Client | Method invocation              |
|--------|--------------------------------|
| RPC    | `{"method": "admin_nodeInfo"}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"admin_nodeInfo","params":[]}
{
    "jsonrpc": "2.0",
    "id": 1,
    "result": {
        "enode": "enode://44826a5d6a55f88a18298bca4773fca5749cdc3a5c9f308aa7d810e9b31123f3e7c5fba0b1d70aac5308426f47df2a128a6747040a3815cc7dd7167d03be320d@[::]:30303",
            "id": "44826a5d6a55f88a18298bca4773fca5749cdc3a5c9f308aa7d810e9b31123f3e7c5fba0b1d70aac5308426f47df2a128a6747040a3815cc7dd7167d03be320d",
            "ip": "::",
            "listenAddr": "[::]:30303",
            "name": "reth/v0.0.1/x86_64-unknown-linux-gnu",
            "ports": {
                "discovery": 30303,
                "listener": 30303
        },
        "protocols": {
            "eth": {
                "difficulty": 17334254859343145000,
                "genesis": "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3",
                "head": "0xb83f73fbe6220c111136aefd27b160bf4a34085c65ba89f24246b3162257c36a",
                "network": 1
            }
        }
    }
}
```

## `admin_peerEvents`, `admin_peerEvents_unsubscribe`

<!-- TODO: This seems to be unimplemented, so it is not really known what the events look like !-->

Subscribe to events received by peers over the network.

Like other subscription methods, this returns the ID of the subscription, which is then used in all events subsequently.

To unsubscribe from peer events, call `admin_peerEvents_unsubscribe`

| Client | Method invocation                |
|--------|----------------------------------|
| RPC    | `{"method": "admin_peerEvents"}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"admin_peerEvents","params":[]}
// responds with subscription ID
{"jsonrpc": "2.0", "id": 1, "result": "0xcd0c3e8af590364c09d0fa6a1210faf5"}
```

[enode]: https://ethereum.org/en/developers/docs/networking-layer/network-addresses/#enode