# `rpc` Namespace

The `rpc` API provides methods to get information about the RPC server itself, such as the enabled namespaces.

## `rpc_modules`

Lists the enabled RPC namespaces and the versions of each.

| Client | Method invocation                         |
|--------|-------------------------------------------|
| RPC    | `{"method": "rpc_modules", "params": []}` |

### Example

```js
// > {"jsonrpc":"2.0","id":1,"method":"rpc_modules","params":[]}
{"jsonrpc":"2.0","id":1,"result":{"txpool":"1.0","eth":"1.0","rpc":"1.0"}}
```