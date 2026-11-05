//! Test to check that the proc macros actually generates documentation.

#![deny(missing_docs)]

use jsonrpsee::core::RpcResult;
use jsonrpsee::proc_macros::rpc;

#[rpc(client, server)]
pub trait ApiWithDocumentation {
	/// Async method.
	#[method(name = "foo")]
	async fn async_method(&self) -> RpcResult<u8>;

	/// Subscription docs.
	#[subscription(name = "sub", unsubscribe = "unsub", item = String)]
	async fn sub(&self);
}

fn main() {}
