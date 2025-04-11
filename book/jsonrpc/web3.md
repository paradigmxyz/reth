# `web3` Namespace

The `web3` API provides utility functions for the web3 client.

## `web3_clientVersion`

Get the web3 client version.


| Client | Method invocation                  |
|--------|------------------------------------|
| RPC    | `{"method": "web3_clientVersion"}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"web3_clientVersion","params":[]}
{"jsonrpc":"2.0","id":1,"result":"reth/v0.0.1/x86_64-unknown-linux-gnu"}
```

## `web3_sha3`

Get the Keccak-256 hash of the given data.

| Client | Method invocation                            |
|--------|----------------------------------------------|
| RPC    | `{"method": "web3_sha3", "params": [bytes]}` |

### Example

```text
// > echo -n "rust is awesome" | xxd -p
7275737420697320617765736f6d65
```

```js
// > {"jsonrpc":"2.0","id":1,"method":"web3_sha3","params":["0x7275737420697320617765736f6d65"]}
{"jsonrpc":"2.0","id":1,"result":"0xe421b3428564a5c509ac118bad93a3b84485ec3f927e214b0c4c23076d4bc4e0"}
```