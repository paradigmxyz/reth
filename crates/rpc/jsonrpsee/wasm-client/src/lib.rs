// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! # jsonrpsee-wasm-client

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg(target_arch = "wasm32")]

pub use jsonrpsee_core::client::Client;
pub use jsonrpsee_types as types;

use std::time::Duration;

use jsonrpsee_client_transport::web;
use jsonrpsee_core::client::async_client::RpcService;
use jsonrpsee_core::client::{Error, IdKind};
use jsonrpsee_core::middleware::{RpcServiceBuilder, layer::RpcLoggerLayer};

type Logger = tower::layer::util::Stack<RpcLoggerLayer, tower::layer::util::Identity>;

/// Builder for [`Client`].
///
/// # Examples
///
/// ```no_run
///
/// use jsonrpsee_wasm_client::WasmClientBuilder;
///
/// #[tokio::main]
/// async fn main() {
///     // build client
///     let client = WasmClientBuilder::default()
///          .build("wss://localhost:443")
///          .await
///          .unwrap();
///
///     // use client....
/// }
///
/// ```
#[derive(Clone, Debug)]
pub struct WasmClientBuilder<L = Logger> {
	id_kind: IdKind,
	max_concurrent_requests: usize,
	max_buffer_capacity_per_subscription: usize,
	request_timeout: Duration,
	service_builder: RpcServiceBuilder<L>,
}

impl Default for WasmClientBuilder {
	fn default() -> Self {
		Self {
			id_kind: IdKind::Number,
			max_concurrent_requests: 256,
			max_buffer_capacity_per_subscription: 1024,
			request_timeout: Duration::from_secs(60),
			service_builder: RpcServiceBuilder::default().rpc_logger(1024),
		}
	}
}

impl WasmClientBuilder {
	/// Create a new WASM client builder.
	pub fn new() -> WasmClientBuilder {
		WasmClientBuilder::default()
	}
}

impl<L> WasmClientBuilder<L> {
	/// See documentation [`ClientBuilder::request_timeout`] (default is 60 seconds).
	pub fn request_timeout(mut self, timeout: Duration) -> Self {
		self.request_timeout = timeout;
		self
	}

	/// See documentation [`ClientBuilder::max_concurrent_requests`] (default is 256).
	pub fn max_concurrent_requests(mut self, max: usize) -> Self {
		self.max_concurrent_requests = max;
		self
	}

	/// See documentation [`ClientBuilder::max_buffer_capacity_per_subscription`] (default is 1024).
	pub fn max_buffer_capacity_per_subscription(mut self, max: usize) -> Self {
		self.max_buffer_capacity_per_subscription = max;
		self
	}

	/// See documentation for [`ClientBuilder::id_format`] (default is Number).
	pub fn id_format(mut self, kind: IdKind) -> Self {
		self.id_kind = kind;
		self
	}

	/// See documentation for [`ClientBuilder::set_rpc_middleware`].
	pub fn set_rpc_middleware<T>(self, middleware: RpcServiceBuilder<T>) -> WasmClientBuilder<T> {
		WasmClientBuilder {
			id_kind: self.id_kind,
			max_concurrent_requests: self.max_concurrent_requests,
			max_buffer_capacity_per_subscription: self.max_buffer_capacity_per_subscription,
			request_timeout: self.request_timeout,
			service_builder: middleware,
		}
	}

	/// Build the client with specified URL to connect to.
	pub async fn build<S>(self, url: impl AsRef<str>) -> Result<Client<S>, Error>
	where
		L: tower::Layer<RpcService, Service = S> + Clone + Send + Sync + 'static,
	{
		let Self {
			id_kind,
			request_timeout,
			max_concurrent_requests,
			max_buffer_capacity_per_subscription,
			service_builder,
		} = self;
		let (sender, receiver) = web::connect(url).await.map_err(|e| Error::Transport(e.into()))?;

		let client = Client::builder()
			.request_timeout(request_timeout)
			.id_format(id_kind)
			.max_buffer_capacity_per_subscription(max_buffer_capacity_per_subscription)
			.max_concurrent_requests(max_concurrent_requests)
			.set_rpc_middleware(service_builder)
			.build_with_wasm(sender, receiver);

		Ok(client)
	}
}
