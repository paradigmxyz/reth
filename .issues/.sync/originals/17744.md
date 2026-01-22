---
title: Allow `tower::Service` as RPC middleware for access to Http Headers
labels:
    - A-sdk
    - C-enhancement
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.994187Z
info:
    author: 0xOsiris
    created_at: 2025-08-06T20:16:15Z
    updated_at: 2025-08-29T07:29:45Z
---

### Describe the feature

The `RpcAddOns` allows for `RpcMiddleware` to be set as middleware on the RPC server, but this type bounds the layer's service to `RpcServiceT` which limits the context of the request that the middleware has access to only the JSON-RPC request. 

```rust
pub trait RpcServiceT {
	/// Response type for `RpcServiceT::call`.
	type MethodResponse;
	/// Response type for `RpcServiceT::notification`.
	type NotificationResponse;
	/// Response type for `RpcServiceT::batch`.
	type BatchResponse;

	/// Processes a single JSON-RPC call, which may be a subscription or regular call.
	fn call<'a>(&self, request: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a;

	/// Processes multiple JSON-RPC calls at once, similar to `RpcServiceT::call`.
	///
	/// This method wraps `RpcServiceT::call` and `RpcServiceT::notification`,
	/// but the root RPC service does not inherently recognize custom implementations
	/// of these methods.
	///
	/// As a result, if you have custom logic for individual calls or notifications,
	/// you must duplicate that implementation in this method or no middleware will be applied
	/// for calls inside the batch.
	fn batch<'a>(&self, requests: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a;

	/// Similar to `RpcServiceT::call` but processes a JSON-RPC notification.
	fn notification<'a>(&self, n: Notification<'a>) -> impl Future<Output = Self::NotificationResponse> + Send + 'a;
}
```

It would be useful to be able to pass an optional `tower::Layer<Service: tower::Service>` as middleware within the `RpcAddOns` such that the service can have context into the http request headers, and body. A valid use case here is passing additional context within the http headers of consensus requests without modifying the JSON-RPC api of the node. e.g. [granting signed authorization within a Fork Choice Updated from the consensus layer](https://github.com/flashbots/rollup-boost/blob/forerunner/p2p-flashblocks/specs/flashblocks_p2p.md)

This is currently not possible while using the Node Builder API AFAIK

### Additional context

`RpcAddOns`
https://github.com/paradigmxyz/reth/blob/b5e65926a0ac4f9a596d14c6e339f519be589ffc/crates/node/builder/src/rpc.rs#L424
