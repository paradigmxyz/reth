---
reth-rpc-api: minor
reth-rpc-builder: minor
reth-rpc-engine-api: minor
reth-rpc: minor
reth-node-builder: minor
---

Moved `reth_newPayload` from the `RethEngineApi` trait into the `RethApi` trait, making `RethApi` generic over `PayloadT: PayloadTypes`. Removed the `RethEngineApi` trait and updated `RpcRegistryInner` and `build_with_auth_server` to accept an optional `beacon_engine_handle` for routing `reth_newPayload` calls.
