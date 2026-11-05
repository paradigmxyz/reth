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

//! RPC Logger layer.

use crate::middleware::{Batch, Notification, RpcServiceT};
use crate::traits::ToJson;

use futures_util::Future;
use jsonrpsee_types::Request;
use serde_json::value::RawValue;
use tracing::Instrument;

/// RPC logger layer.
#[derive(Copy, Clone, Debug)]
pub struct RpcLoggerLayer(u32);

impl RpcLoggerLayer {
	/// Create a new logging layer.
	pub fn new(max: u32) -> Self {
		Self(max)
	}
}

impl<S> tower::Layer<S> for RpcLoggerLayer {
	type Service = RpcLogger<S>;

	fn layer(&self, service: S) -> Self::Service {
		RpcLogger { service, max: self.0 }
	}
}

/// A middleware that logs each RPC call and response.
#[derive(Debug, Clone)]
pub struct RpcLogger<S> {
	max: u32,
	service: S,
}

impl<S> RpcServiceT for RpcLogger<S>
where
	S: RpcServiceT + Send + Sync + Clone + 'static,
	S::MethodResponse: ToJson,
	S::BatchResponse: ToJson,
{
	type MethodResponse = S::MethodResponse;
	type NotificationResponse = S::NotificationResponse;
	type BatchResponse = S::BatchResponse;

	#[tracing::instrument(name = "method_call", skip_all, fields(method = request.method_name()), level = "trace")]
	fn call<'a>(&self, request: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
		let json = serde_json::value::to_raw_value(&request);
		let json_str = unwrap_json_str_or_invalid(&json);
		tracing::trace!(target: "jsonrpsee", "request = {}", truncate_at_char_boundary(json_str, self.max as usize));

		let service = self.service.clone();
		let max = self.max;

		async move {
			let rp = service.call(request).await;

			let json = rp.to_json();
			let json_str = unwrap_json_str_or_invalid(&json);
			tracing::trace!(target: "jsonrpsee", "response = {}", truncate_at_char_boundary(json_str, max as usize));
			rp
		}
		.in_current_span()
	}

	#[tracing::instrument(name = "batch", skip_all, fields(method = "batch"), level = "trace")]
	fn batch<'a>(&self, batch: Batch<'a>) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
		let json = serde_json::value::to_raw_value(&batch);
		let json_str = unwrap_json_str_or_invalid(&json);
		tracing::trace!(target: "jsonrpsee", "batch request = {}", truncate_at_char_boundary(json_str, self.max as usize));
		let service = self.service.clone();
		let max = self.max;

		async move {
			let rp = service.batch(batch).await;

			let json = rp.to_json();
			let json_str = unwrap_json_str_or_invalid(&json);
			tracing::trace!(target: "jsonrpsee", "batch response = {}", truncate_at_char_boundary(json_str, max as usize));

			rp
		}
		.in_current_span()
	}

	#[tracing::instrument(name = "notification", skip_all, fields(method = &*n.method), level = "trace")]
	fn notification<'a>(&self, n: Notification<'a>) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
		let json = serde_json::value::to_raw_value(&n);
		let json_str = unwrap_json_str_or_invalid(&json);
		tracing::trace!(target: "jsonrpsee", "notification request = {}", truncate_at_char_boundary(json_str, self.max as usize));

		self.service.notification(n).in_current_span()
	}
}

fn unwrap_json_str_or_invalid(json: &Result<Box<RawValue>, serde_json::Error>) -> &str {
	match json {
		Ok(s) => s.get(),
		Err(_) => "<invalid JSON>",
	}
}

/// Find the next char boundary to truncate at.
fn truncate_at_char_boundary(s: &str, max: usize) -> &str {
	if s.len() < max {
		return s;
	}

	match s.char_indices().nth(max) {
		None => s,
		Some((idx, _)) => &s[..idx],
	}
}

#[cfg(test)]
mod tests {
	use super::truncate_at_char_boundary;

	#[test]
	fn truncate_at_char_boundary_works() {
		assert_eq!(truncate_at_char_boundary("ボルテックス", 0), "");
		assert_eq!(truncate_at_char_boundary("ボルテックス", 4), "ボルテッ");
		assert_eq!(truncate_at_char_boundary("ボルテックス", 100), "ボルテックス");
		assert_eq!(truncate_at_char_boundary("hola-hola", 4), "hola");
	}
}
