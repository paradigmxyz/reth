//! Test that calling a deprecated method will generate warnings at compile-time.

// Treat warnings as errors to fail the build.
#![deny(warnings)]

use std::net::SocketAddr;

use jsonrpsee::core::{async_trait, RpcResult};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::ServerBuilder;
use jsonrpsee::ws_client::*;

#[rpc(client, server)]
pub trait Deprecated {
	// Deprecated method that is called by the client.
	#[deprecated(since = "0.5.0", note = "please use `new_method` instead")]
	#[method(name = "foo")]
	async fn async_method(&self) -> RpcResult<u8>;

	// Deprecated methods that are not called should not generate warnings.
	#[deprecated(since = "0.5.0", note = "please use `new_method` instead")]
	#[method(name = "foo_unused")]
	async fn async_method_unused(&self) -> RpcResult<u8>;

	// If the method is not marked as deprecated, should not generate warnings.
	#[method(name = "bar")]
	fn sync_method(&self) -> RpcResult<u8>;
}

pub struct DeprecatedServerImpl;

#[async_trait]
impl DeprecatedServer for DeprecatedServerImpl {
	async fn async_method(&self) -> RpcResult<u8> {
		Ok(16u8)
	}

	async fn async_method_unused(&self) -> RpcResult<u8> {
		Ok(32u8)
	}

	fn sync_method(&self) -> RpcResult<u8> {
		Ok(64u8)
	}
}

pub async fn server() -> SocketAddr {
	let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
	let addr = server.local_addr().unwrap();

	server.start(DeprecatedServerImpl.into_rpc());

	addr
}

#[tokio::main]
async fn main() {
	let server_addr = server().await;
	let server_url = format!("ws://{}", server_addr);
	let client = WsClientBuilder::default().build(&server_url).await.unwrap();

	// Calling this method should generate an warning.
	assert_eq!(client.async_method().await.unwrap(), 16);
	// Note: `async_method_unused` is not called, and should not generate warnings.
	assert_eq!(client.sync_method().await.unwrap(), 64);
}
