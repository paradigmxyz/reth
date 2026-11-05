//! Example of using proc macro to generate working client and server with bounds applied.

use std::net::SocketAddr;

use jsonrpsee::core::{async_trait, RpcResult, SubscriptionResult};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::ServerBuilder;
use jsonrpsee::ws_client::*;

pub trait Config {
	type Hash: Send + Sync + 'static;
	type NotUsed;
}

type ExampleHash = [u8; 32];
impl Config for ExampleHash {
	type Hash = Self;
	type NotUsed = ();
}

/// Client only RPC.
#[rpc(client, namespace = "foo", client_bounds(Conf::Hash: jsonrpsee::core::DeserializeOwned))]
pub trait MyRpcC<Conf: Config> {
	#[method(name = "bar")]
	fn method(&self) -> RpcResult<Conf::Hash>;
}

/// Server only RPC.
#[rpc(server, namespace = "foo", server_bounds(Conf::Hash: jsonrpsee::core::Serialize + Clone))]
pub trait MyRpcS<Conf: Config> {
	#[method(name = "bar")]
	fn method(&self) -> RpcResult<Conf::Hash>;
}

/// Client and server RPC.
#[rpc(server, client, namespace = "foo", client_bounds(Conf::Hash: jsonrpsee::core::DeserializeOwned), server_bounds(Conf::Hash: jsonrpsee::core::Serialize + Clone))]
pub trait MyRpcSC<Conf: Config> {
	#[method(name = "bar")]
	fn method(&self) -> RpcResult<Conf::Hash>;
}

/// Trait to ensure that the trait bounds are correct.
#[rpc(client, server, namespace = "generic_call")]
pub trait OnlyGenericCall<I, R> {
	#[method(name = "getHeader")]
	fn call(&self, input: I) -> RpcResult<R>;
}

/// Trait to ensure that the trait bounds are correct.
#[rpc(client, server, namespace = "generic_sub")]
pub trait OnlyGenericSubscription<Input, R> {
	/// Get header of a relay chain block.
	#[subscription(name = "sub", unsubscribe = "unsub", item = Vec<R>)]
	async fn sub(&self, hash: Input) -> SubscriptionResult;
}

/// Trait to ensure that the trait bounds are correct.
#[rpc(client, server, namespace = "generic_with_where_clause")]
pub trait GenericWhereClause<I, R>
where
	I: std::fmt::Debug,
	R: Copy + Clone,
{
	#[method(name = "getHeader")]
	fn call(&self, input: I) -> RpcResult<R>;
}

/// Trait to ensure that the trait bounds are correct.
#[rpc(client, server, namespace = "generic_with_where_clause")]
pub trait GenericWhereClauseWithTypeBoundsToo<I: Copy + Clone, R>
where
	I: std::fmt::Debug,
	R: Copy + Clone,
{
	#[method(name = "getHeader")]
	fn call(&self, input: I) -> RpcResult<R>;
}

#[rpc(client, server, namespace = "chain")]
pub trait ChainApi<Number, Hash, Header, SignedBlock> {
	/// Get header of a relay chain block.
	#[method(name = "getHeader")]
	fn header(&self, hash: Option<Hash>) -> RpcResult<Option<Header>>;

	/// Get header and body of a relay chain block.
	#[method(name = "getBlock")]
	async fn block(&self, hash: Option<Hash>) -> RpcResult<Option<SignedBlock>>;

	/// Get hash of the n-th block in the canon chain.
	///
	/// By default returns latest block hash.
	#[method(name = "getBlockHash")]
	fn block_hash(&self, hash: Hash) -> RpcResult<Option<Hash>>;

	/// Get hash of the last finalized block in the canon chain.
	#[method(name = "getFinalizedHead")]
	fn finalized_head(&self) -> RpcResult<Hash>;

	/// All head subscription
	#[subscription(name = "subscribeAllHeads", item = Header)]
	async fn subscribe_all_heads(&self, hash: Hash) -> SubscriptionResult;
}

/// Implementation for the `MyRpcS` trait (server only).
pub struct ServerOnlyImpl;
#[async_trait]
impl MyRpcSServer<ExampleHash> for ServerOnlyImpl {
	fn method(&self) -> RpcResult<<ExampleHash as Config>::Hash> {
		Ok([0u8; 32])
	}
}

/// Implementation for the `MyRpcSC` trait (client server rpc).
pub struct ServerClientServerImpl;
#[async_trait]
impl MyRpcSCServer<ExampleHash> for ServerClientServerImpl {
	fn method(&self) -> RpcResult<<ExampleHash as Config>::Hash> {
		Ok([0u8; 32])
	}
}

pub async fn websocket_servers() -> (SocketAddr, SocketAddr) {
	// Start server from `MyRpcS` trait.
	let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
	let addr_server_only = server.local_addr().unwrap();
	let server_handle = server.start(ServerOnlyImpl.into_rpc());

	tokio::spawn(server_handle.stopped());

	// Start server from `MyRpcSC` trait.
	let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
	let addr_server_client = server.local_addr().unwrap();
	let server_handle = server.start(ServerClientServerImpl.into_rpc());

	tokio::spawn(server_handle.stopped());

	(addr_server_only, addr_server_client)
}

#[tokio::main]
async fn main() {
	let (server_addr, server_addr_w_client) = websocket_servers().await;
	let (server_url, server_w_client_url) = (format!("ws://{}", server_addr), format!("ws://{}", server_addr_w_client));
	let client = WsClientBuilder::default().build(&server_url).await.unwrap();
	let client_second = WsClientBuilder::default().build(&server_w_client_url).await.unwrap();

	// Use `MyRpcC` client to communicate to the `MyRpcS` server.
	assert_eq!(MyRpcCClient::<ExampleHash>::method(&client).await.unwrap(), [0u8; 32]);
	// Use `MyRpcC` client to communicate to the `MyRpcSC` server.
	assert_eq!(MyRpcCClient::<ExampleHash>::method(&client_second).await.unwrap(), [0u8; 32]);

	// Use `MyRpcSC` client to communicate to the `MyRpcS` server.
	assert_eq!(MyRpcCClient::<ExampleHash>::method(&client).await.unwrap(), [0u8; 32]);
	// Use `MyRpcSC` client to communicate to the `MyRpcSC` server.
	assert_eq!(MyRpcSCClient::<ExampleHash>::method(&client_second).await.unwrap(), [0u8; 32]);
}
