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

## Handling Responses During Syncing

When interacting with the RPC server while it is still syncing, some RPC requests may return an empty or null response, while others return the expected results. This behavior can be observed due to the asynchronous nature of the syncing process and the availability of required data. Notably, endpoints that rely on specific stages of the syncing process, such as the execution stage, might not be available until those stages are complete.

It's important to understand that during pipeline sync, some endpoints may not be accessible until the necessary data is fully synchronized. For instance, the `eth_getBlockReceipts` endpoint is only expected to return valid data after the execution stage, where receipts are generated, has completed. As a result, certain RPC requests may return empty or null responses until the respective stages are finished.

This behavior is intrinsic to how the syncing mechanism works and is not indicative of an issue or bug. If you encounter such responses while the node is still syncing, it's recommended to wait until the sync process is complete to ensure accurate and expected RPC responses.
