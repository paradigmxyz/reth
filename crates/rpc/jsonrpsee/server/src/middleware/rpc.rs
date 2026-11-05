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

//! JSON-RPC service middleware.

pub use jsonrpsee_core::middleware::*;
pub use jsonrpsee_core::server::MethodResponse;

use std::sync::Arc;

use crate::ConnectionId;
use jsonrpsee_core::server::{
	BatchResponseBuilder, BoundedSubscriptions, MethodCallback, MethodSink, Methods, SubscriptionState,
};
use jsonrpsee_core::traits::IdProvider;
use jsonrpsee_types::ErrorObject;
use jsonrpsee_types::error::{ErrorCode, reject_too_many_subscriptions};

/// JSON-RPC service middleware.
#[derive(Clone, Debug)]
pub struct RpcService {
	conn_id: ConnectionId,
	methods: Methods,
	max_response_body_size: usize,
	cfg: RpcServiceCfg,
}

/// Configuration of the RpcService.
#[derive(Clone, Debug)]
pub(crate) enum RpcServiceCfg {
	/// The server supports only calls.
	OnlyCalls,
	/// The server supports both method calls and subscriptions.
	CallsAndSubscriptions {
		bounded_subscriptions: BoundedSubscriptions,
		sink: MethodSink,
		id_provider: Arc<dyn IdProvider>,
		_pending_calls: tokio::sync::mpsc::Sender<()>,
	},
}

impl RpcService {
	/// Create a new service.
	pub(crate) fn new(
		methods: Methods,
		max_response_body_size: usize,
		conn_id: ConnectionId,
		cfg: RpcServiceCfg,
	) -> Self {
		Self { methods, max_response_body_size, conn_id, cfg }
	}
}

impl RpcServiceT for RpcService {
	type BatchResponse = MethodResponse;
	type MethodResponse = MethodResponse;
	type NotificationResponse = MethodResponse;

	fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
		let conn_id = self.conn_id;
		let max_response_body_size = self.max_response_body_size;

		let Request { id, method, params, extensions, .. } = req;
		let params = jsonrpsee_types::Params::new(params.as_ref().map(|p| serde_json::value::RawValue::get(p)));

		match self.methods.method_with_name(&method) {
			None => {
				let rp =
					MethodResponse::error(id, ErrorObject::from(ErrorCode::MethodNotFound)).with_extensions(extensions);
				ResponseFuture::ready(rp)
			}
			Some((_name, method)) => match method {
				MethodCallback::Async(callback) => {
					let params = params.into_owned();
					let id = id.into_owned();
					let fut = (callback)(id, params, conn_id, max_response_body_size, extensions);

					ResponseFuture::future(fut)
				}
				MethodCallback::Sync(callback) => {
					let rp = (callback)(id, params, max_response_body_size, extensions);
					ResponseFuture::ready(rp)
				}
				MethodCallback::Subscription(callback) => {
					let RpcServiceCfg::CallsAndSubscriptions {
						bounded_subscriptions,
						sink,
						id_provider,
						_pending_calls,
					} = self.cfg.clone()
					else {
						tracing::warn!("Subscriptions not supported");
						let rp = MethodResponse::error(id, ErrorObject::from(ErrorCode::InternalError))
							.with_extensions(extensions);
						return ResponseFuture::ready(rp);
					};

					if let Some(p) = bounded_subscriptions.acquire() {
						let conn_state =
							SubscriptionState { conn_id, id_provider: &*id_provider.clone(), subscription_permit: p };

						let fut = (callback)(id.clone(), params, sink, conn_state, extensions);
						ResponseFuture::future(fut)
					} else {
						let max = bounded_subscriptions.max();
						let rp =
							MethodResponse::error(id, reject_too_many_subscriptions(max)).with_extensions(extensions);
						ResponseFuture::ready(rp)
					}
				}
				MethodCallback::Unsubscription(callback) => {
					// Don't adhere to any resource or subscription limits; always let unsubscribing happen!

					let RpcServiceCfg::CallsAndSubscriptions { .. } = self.cfg else {
						tracing::warn!("Subscriptions not supported");
						let rp = MethodResponse::error(id, ErrorObject::from(ErrorCode::InternalError))
							.with_extensions(extensions);
						return ResponseFuture::ready(rp);
					};

					let rp = callback(id, params, conn_id, max_response_body_size, extensions);
					ResponseFuture::ready(rp)
				}
			},
		}
	}

	fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
		let mut batch_rp = BatchResponseBuilder::new_with_limit(self.max_response_body_size);
		let service = self.clone();
		async move {
			let mut got_notification = false;

			for batch_entry in batch.into_iter() {
				match batch_entry {
					Ok(BatchEntry::Call(req)) => {
						let rp = service.call(req).await;
						if let Err(err) = batch_rp.append(rp) {
							return err;
						}
					}
					Ok(BatchEntry::Notification(n)) => {
						got_notification = true;
						service.notification(n).await;
					}
					Err(err) => {
						let (err, id) = err.into_parts();
						let rp = MethodResponse::error(id, err);
						if let Err(err) = batch_rp.append(rp) {
							return err;
						}
					}
				}
			}

			// If the batch is empty and we got a notification, we return an empty response.
			if batch_rp.is_empty() && got_notification {
				MethodResponse::notification()
			}
			// An empty batch is regarded as an invalid request here.
			else {
				MethodResponse::from_batch(batch_rp.finish())
			}
		}
	}

	fn notification<'a>(&self, n: Notification<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
		// The notification should not be replied to with a response
		// but we propogate the extensions to the response which can be useful
		// for example HTTP transport to set the headers.
		async move { MethodResponse::notification().with_extensions(n.extensions) }
	}
}
