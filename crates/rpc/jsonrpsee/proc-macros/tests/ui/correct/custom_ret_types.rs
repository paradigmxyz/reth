//! Example of using custom response type.

use std::net::SocketAddr;

use jsonrpsee::core::{async_trait, ClientError, Serialize};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{IntoResponse, ResponsePayload, ServerBuilder};
use jsonrpsee::ws_client::*;

// Serialize impl is not used as the responses are sent out as error.
#[derive(Serialize, Clone)]
pub enum CustomError {
	One,
	Two { custom_data: u32 },
}

impl IntoResponse for CustomError {
	type Output = Self;

	fn into_response(self) -> ResponsePayload<'static, Self::Output> {
		let code = match &self {
			CustomError::One => 101,
			CustomError::Two { .. } => 102,
		};
		let data = match &self {
			CustomError::One => None,
			CustomError::Two { custom_data } => Some(serde_json::json!({ "customData": custom_data })),
		};

		let data = data.map(|val| serde_json::value::to_raw_value(&val).unwrap());

		let error_object = jsonrpsee::types::ErrorObjectOwned::owned(code, "custom_error", data);
		ResponsePayload::error(error_object)
	}
}

#[rpc(server, namespace = "foo")]
pub trait Rpc {
	#[method(name = "async_method1")]
	async fn async_method1(&self) -> CustomError;

	#[method(name = "async_method2")]
	async fn async_method2(&self, x: u32) -> CustomError;

	#[method(name = "sync_method1")]
	fn method1(&self) -> CustomError;

	#[method(name = "sync_method2")]
	fn method2(&self, x: u32) -> CustomError;

	#[method(name = "blocking_method1", blocking)]
	fn blocking_method1(&self) -> CustomError;

	#[method(name = "blocking_method2", blocking)]
	fn blocking_method2(&self, x: u32) -> CustomError;
}

pub struct RpcServerImpl;

#[async_trait]
impl RpcServer for RpcServerImpl {
	async fn async_method1(&self) -> CustomError {
		CustomError::One
	}

	async fn async_method2(&self, x: u32) -> CustomError {
		CustomError::Two { custom_data: x }
	}

	fn method1(&self) -> CustomError {
		CustomError::One
	}

	fn method2(&self, x: u32) -> CustomError {
		CustomError::Two { custom_data: x }
	}

	fn blocking_method1(&self) -> CustomError {
		CustomError::One
	}

	fn blocking_method2(&self, x: u32) -> CustomError {
		CustomError::Two { custom_data: x }
	}
}

// TODO: https://github.com/paritytech/jsonrpsee/issues/1067
//
// The client accepts only return types that are `Result<T, ClientError>`.
#[rpc(client, namespace = "foo")]
pub trait RpcClient {
	#[method(name = "async_method1")]
	async fn async_method1(&self) -> RpcResult<serde_json::Value>;

	#[method(name = "async_method2")]
	async fn async_method2(&self, x: u32) -> Result<serde_json::Value, ()>;

	#[method(name = "sync_method1")]
	async fn sync_method1(&self) -> RpcResult<serde_json::Value>;

	#[method(name = "sync_method2")]
	async fn sync_method2(&self, x: u32) -> Result<serde_json::Value, ()>;

	#[method(name = "blocking_method1")]
	async fn blocking_method1(&self) -> RpcResult<serde_json::Value>;

	#[method(name = "blocking_method2")]
	async fn blocking_method2(&self, x: u32) -> Result<serde_json::Value, ()>;
}

pub async fn server() -> SocketAddr {
	let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
	let addr = server.local_addr().unwrap();
	let server_handle = server.start(RpcServerImpl.into_rpc());

	tokio::spawn(server_handle.stopped());

	addr
}

#[tokio::main]
async fn main() {
	let server_addr = server().await;
	let server_url = format!("ws://{}", server_addr);
	let client = WsClientBuilder::default().build(&server_url).await.unwrap();

	let error = client.async_method1().await.unwrap_err();
	assert_method1(error);

	let error = client.async_method2(123).await.unwrap_err();
	assert_method2(error);

	let error = client.sync_method1().await.unwrap_err();
	assert_method1(error);

	let error = client.sync_method2(123).await.unwrap_err();
	assert_method2(error);

	let error = client.blocking_method1().await.unwrap_err();
	assert_method1(error);

	let error = client.blocking_method2(123).await.unwrap_err();
	assert_method2(error);
}

fn assert_method1(error: ClientError) {
	let get_error_object = |err| match err {
		ClientError::Call(object) => object,
		_ => panic!("wrong error kind: {:?}", err),
	};

	let error_object = get_error_object(error);
	assert_eq!(error_object.code(), 101);
	assert_eq!(error_object.message(), "custom_error");
	assert!(error_object.data().is_none());
}

fn assert_method2(error: ClientError) {
	let get_error_object = |err| match err {
		ClientError::Call(object) => object,
		_ => panic!("wrong error kind: {:?}", err),
	};

	let error_object = get_error_object(error);
	assert_eq!(error_object.code(), 102);
	assert_eq!(error_object.message(), "custom_error");
	assert_eq!(error_object.data().unwrap().get(), r#"{"customData":123}"#);
}
