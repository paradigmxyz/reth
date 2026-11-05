//! Middleware for the RPC service.

pub mod layer;

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::future::Either;
use jsonrpsee_types::{ErrorObject, Id};
use pin_project::pin_project;
use serde::Serialize;
use serde::ser::SerializeSeq;
use serde_json::value::RawValue;
use tower::layer::LayerFn;
use tower::layer::util::{Identity, Stack};

/// Re-export types from `jsonrpsee_types` crate for convenience.
pub type Notification<'a> = jsonrpsee_types::Notification<'a, Option<Cow<'a, RawValue>>>;
/// Re-export types from `jsonrpsee_types` crate for convenience.
pub use jsonrpsee_types::{Extensions, Request};

/// Error response that can used to indicate an error in JSON-RPC request batch request.
/// This is used in the [`Batch`] type to indicate an error in the batch entry.
#[derive(Debug)]
pub struct BatchEntryErr<'a>(jsonrpsee_types::Response<'a, ()>);

impl<'a> BatchEntryErr<'a> {
	/// Create a new error response.
	pub fn new(id: Id<'a>, err: ErrorObject<'a>) -> Self {
		let payload = jsonrpsee_types::ResponsePayload::Error(err);
		let response = jsonrpsee_types::Response::new(payload, id);
		Self(response)
	}

	/// Get the parts of the error response.q
	pub fn into_parts(self) -> (ErrorObject<'a>, Id<'a>) {
		let err = match self.0.payload {
			jsonrpsee_types::ResponsePayload::Error(err) => err,
			_ => unreachable!("BatchEntryErr can only be created from error payload; qed"),
		};
		(err, self.0.id)
	}
}

/// A batch of JSON-RPC requests.
#[derive(Debug, Default)]
pub struct Batch<'a> {
	inner: Vec<Result<BatchEntry<'a>, BatchEntryErr<'a>>>,
	extensions: Option<Extensions>,
}

impl std::fmt::Display for Batch<'_> {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let fmt = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
		f.write_str(&fmt)
	}
}

impl<'a> IntoIterator for Batch<'a> {
	type Item = Result<BatchEntry<'a>, BatchEntryErr<'a>>;
	type IntoIter = std::vec::IntoIter<Self::Item>;

	fn into_iter(self) -> Self::IntoIter {
		self.inner.into_iter()
	}
}

impl<'a> Batch<'a> {
	/// Create a new batch from a vector of batch entries.
	pub fn from(entries: Vec<Result<BatchEntry<'a>, BatchEntryErr<'a>>>) -> Self {
		Self { inner: entries, extensions: None }
	}

	/// Create a new empty batch.
	pub fn new() -> Self {
		Self { inner: Vec::new(), extensions: None }
	}

	/// Create a new empty batch with the at least capacity.
	pub fn with_capacity(capacity: usize) -> Self {
		Self { inner: Vec::with_capacity(capacity), extensions: None }
	}

	/// Push a new batch entry to the batch.
	pub fn push(&mut self, req: Request<'a>) {
		match self.extensions {
			Some(ref mut ext) => {
				ext.extend(req.extensions().clone());
			}
			None => {
				self.extensions = Some(req.extensions().clone());
			}
		};

		self.inner.push(Ok(BatchEntry::Call(req)));
	}

	/// Get the length of the batch.
	pub fn len(&self) -> usize {
		self.inner.len()
	}

	/// Returns whether the batch is empty.
	pub fn is_empty(&self) -> bool {
		self.inner.is_empty()
	}

	/// Get an iterator over the batch.
	pub fn iter(&self) -> impl Iterator<Item = &Result<BatchEntry<'a>, BatchEntryErr<'a>>> {
		self.inner.iter()
	}

	/// Get a mutable iterator over the batch.
	pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Result<BatchEntry<'a>, BatchEntryErr<'a>>> {
		self.inner.iter_mut()
	}

	/// Consume the batch and and return the parts.
	pub fn into_extensions(self) -> Extensions {
		match self.extensions {
			Some(ext) => ext,
			None => self.extensions_from_iter(),
		}
	}

	/// Get a reference to the extensions of the batch.
	pub fn extensions(&mut self) -> &Extensions {
		if self.extensions.is_none() {
			self.extensions = Some(self.extensions_from_iter());
		}

		self.extensions.as_ref().expect("Extensions inserted above; qed")
	}

	/// Get a mutable reference to the extensions of the batch.
	pub fn extensions_mut(&mut self) -> &mut Extensions {
		if self.extensions.is_none() {
			self.extensions = Some(self.extensions_from_iter());
		}

		self.extensions.as_mut().expect("Extensions inserted above; qed")
	}

	fn extensions_from_iter(&self) -> Extensions {
		let mut ext = Extensions::new();
		for entry in self.inner.iter().flatten() {
			ext.extend(entry.extensions().clone());
		}
		ext
	}
}

impl Serialize for Batch<'_> {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		let mut seq = serializer.serialize_seq(Some(self.inner.len()))?;
		for entry in &self.inner {
			match entry {
				Ok(entry) => seq.serialize_element(entry)?,
				Err(err) => seq.serialize_element(&err.0)?,
			}
		}
		seq.end()
	}
}

#[derive(Debug, Clone)]
/// A marker type to indicate that the request is a subscription for the [`RpcServiceT::call`] method.
pub struct IsSubscription {
	sub_id: Id<'static>,
	unsub_id: Id<'static>,
	unsub_method: String,
}

impl IsSubscription {
	/// Create a new [`IsSubscription`] instance.
	pub fn new(sub_id: Id<'static>, unsub_id: Id<'static>, unsub_method: String) -> Self {
		Self { sub_id, unsub_id, unsub_method }
	}

	/// Get the request id of the subscription calls.
	pub fn sub_req_id(&self) -> Id<'static> {
		self.sub_id.clone()
	}

	/// Get the request id of the unsubscription call.
	pub fn unsub_req_id(&self) -> Id<'static> {
		self.unsub_id.clone()
	}

	/// Get the unsubscription method name.
	pub fn unsubscribe_method(&self) -> &str {
		&self.unsub_method
	}
}

/// An extension type for the [`RpcServiceT::batch`] for the expected id range of the batch entries.
#[derive(Debug, Clone)]
pub struct IsBatch {
	/// The range of ids for the batch entries.
	pub id_range: std::ops::Range<u64>,
}

/// A batch entry specific for the [`RpcServiceT::batch`] method to support both
/// method calls and notifications.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum BatchEntry<'a> {
	/// A regular JSON-RPC call.
	Call(Request<'a>),
	/// A JSON-RPC notification.
	Notification(Notification<'a>),
}

impl<'a> BatchEntry<'a> {
	/// Get a reference to extensions of the batch entry.
	pub fn extensions(&self) -> &Extensions {
		match self {
			BatchEntry::Call(req) => req.extensions(),
			BatchEntry::Notification(n) => n.extensions(),
		}
	}

	/// Get a mut reference to extensions of the batch entry.
	pub fn extensions_mut(&mut self) -> &mut Extensions {
		match self {
			BatchEntry::Call(req) => req.extensions_mut(),
			BatchEntry::Notification(n) => n.extensions_mut(),
		}
	}

	/// Get the method name of the batch entry.
	pub fn method_name(&self) -> &str {
		match self {
			BatchEntry::Call(req) => req.method_name(),
			BatchEntry::Notification(n) => n.method_name(),
		}
	}

	/// Set the method name of the batch entry.
	pub fn set_method_name(&mut self, name: impl Into<Cow<'a, str>>) {
		match self {
			BatchEntry::Call(req) => {
				req.method = name.into();
			}
			BatchEntry::Notification(n) => {
				n.method = name.into();
			}
		}
	}

	/// Get the params of the batch entry (may be empty).
	pub fn params(&self) -> Option<&Cow<'a, RawValue>> {
		match self {
			BatchEntry::Call(req) => req.params.as_ref(),
			BatchEntry::Notification(n) => n.params.as_ref(),
		}
	}

	/// Set the params of the batch entry.
	pub fn set_params(&mut self, params: Option<Box<RawValue>>) {
		match self {
			BatchEntry::Call(req) => {
				req.params = params.map(Cow::Owned);
			}
			BatchEntry::Notification(n) => {
				n.params = params.map(Cow::Owned);
			}
		}
	}

	/// Consume the batch entry and extract the extensions.
	pub fn into_extensions(self) -> Extensions {
		match self {
			BatchEntry::Call(req) => req.extensions,
			BatchEntry::Notification(n) => n.extensions,
		}
	}
}

/// Represent a JSON-RPC service that can process JSON-RPC calls, notifications, and batch requests.
///
/// This trait is similar to [`tower::Service`] but it's specialized for JSON-RPC operations.
///
/// The response type is a future that resolves to a `Result<R, E>` mainly because this trait is
/// intended to by used by both client and server implementations.
///
/// In the server implementation, the error is infallible but in the client implementation, the error
/// can occur due to I/O errors or JSON-RPC protocol errors.
///
/// Such that server implementations must use `std::convert::Infallible` as the error type because
/// the underlying service requires that and otherwise one will get a compiler error that tries to
/// explain that.
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

/// Similar to [`tower::ServiceBuilder`] but doesn't
/// support any tower middleware implementations.
#[derive(Debug, Clone)]
pub struct RpcServiceBuilder<L>(tower::ServiceBuilder<L>);

impl Default for RpcServiceBuilder<Identity> {
	fn default() -> Self {
		RpcServiceBuilder(tower::ServiceBuilder::new())
	}
}

impl RpcServiceBuilder<Identity> {
	/// Create a new [`RpcServiceBuilder`].
	pub fn new() -> Self {
		Self(tower::ServiceBuilder::new())
	}
}

impl<L> RpcServiceBuilder<L> {
	/// Optionally add a new layer `T` to the [`RpcServiceBuilder`].
	///
	/// See the documentation for [`tower::ServiceBuilder::option_layer`] for more details.
	pub fn option_layer<T>(self, layer: Option<T>) -> RpcServiceBuilder<Stack<layer::Either<T, Identity>, L>> {
		let layer =
			if let Some(layer) = layer { layer::Either::Left(layer) } else { layer::Either::Right(Identity::new()) };
		RpcServiceBuilder(self.0.layer(layer))
	}

	/// Add a new layer `T` to the [`RpcServiceBuilder`].
	///
	/// See the documentation for [`tower::ServiceBuilder::layer`] for more details.
	pub fn layer<T>(self, layer: T) -> RpcServiceBuilder<Stack<T, L>> {
		RpcServiceBuilder(self.0.layer(layer))
	}

	/// Add a [`tower::Layer`] built from a function that accepts a service and returns another service.
	///
	/// See the documentation for [`tower::ServiceBuilder::layer_fn`] for more details.
	pub fn layer_fn<F>(self, f: F) -> RpcServiceBuilder<Stack<LayerFn<F>, L>> {
		RpcServiceBuilder(self.0.layer_fn(f))
	}

	/// Add a logging layer to [`RpcServiceBuilder`]
	///
	/// This logs each request and response for every call.
	///
	pub fn rpc_logger(self, max_log_len: u32) -> RpcServiceBuilder<Stack<layer::RpcLoggerLayer, L>> {
		RpcServiceBuilder(self.0.layer(layer::RpcLoggerLayer::new(max_log_len)))
	}

	/// Wrap the service `S` with the middleware.
	pub fn service<S>(&self, service: S) -> L::Service
	where
		L: tower::Layer<S>,
	{
		self.0.service(service)
	}
}

/// Response which may be ready or a future.
#[derive(Debug)]
#[pin_project]
pub struct ResponseFuture<F, R>(#[pin] futures_util::future::Either<F, std::future::Ready<R>>);

impl<F, R> ResponseFuture<F, R> {
	/// Returns a future that resolves to a response.
	pub fn future(f: F) -> ResponseFuture<F, R> {
		ResponseFuture(Either::Left(f))
	}

	/// Return a response which is already computed.
	pub fn ready(response: R) -> ResponseFuture<F, R> {
		ResponseFuture(Either::Right(std::future::ready(response)))
	}
}

impl<F, R> Future for ResponseFuture<F, R>
where
	F: Future<Output = R>,
{
	type Output = F::Output;

	fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
		match self.project().0.poll(cx) {
			Poll::Ready(rp) => Poll::Ready(rp),
			Poll::Pending => Poll::Pending,
		}
	}
}

#[cfg(test)]
mod tests {
	use jsonrpsee_types::{ErrorCode, ErrorObject};

	#[test]
	fn serialize_batch_entry() {
		use super::{BatchEntry, Notification, Request};
		use jsonrpsee_types::Id;

		let req = Request::borrowed("say_hello", None, Id::Number(1));
		let batch_entry = BatchEntry::Call(req.clone());
		assert_eq!(
			serde_json::to_string(&batch_entry).unwrap(),
			"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"say_hello\"}",
		);

		let notification = Notification::new("say_hello".into(), None);
		let batch_entry = BatchEntry::Notification(notification.clone());
		assert_eq!(
			serde_json::to_string(&batch_entry).unwrap(),
			"{\"jsonrpc\":\"2.0\",\"method\":\"say_hello\",\"params\":null}",
		);
	}

	#[test]
	fn serialize_batch_works() {
		use super::{Batch, BatchEntry, BatchEntryErr, Notification, Request};
		use jsonrpsee_types::Id;

		let req = Request::borrowed("say_hello", None, Id::Number(1));
		let notification = Notification::new("say_hello".into(), None);
		let batch = Batch::from(vec![
			Ok(BatchEntry::Call(req)),
			Ok(BatchEntry::Notification(notification)),
			Err(BatchEntryErr::new(Id::Number(2), ErrorObject::from(ErrorCode::InvalidRequest))),
		]);
		assert_eq!(
			serde_json::to_string(&batch).unwrap(),
			r#"[{"jsonrpc":"2.0","id":1,"method":"say_hello"},{"jsonrpc":"2.0","method":"say_hello","params":null},{"jsonrpc":"2.0","id":2,"error":{"code":-32600,"message":"Invalid request"}}]"#,
		);
	}
}
