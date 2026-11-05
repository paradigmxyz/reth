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

//! Shared utilities for `jsonrpsee` clients.

cfg_async_client! {
	pub mod async_client;
	pub use async_client::{Client, ClientBuilder};
}

pub mod error;

pub use error::Error;

use std::fmt;
use std::future::Future;
use std::ops::{Deref, Range};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{self, Poll};

use crate::params::BatchRequestBuilder;
use crate::traits::{ToJson, ToRpcParams};

use core::marker::PhantomData;
use futures_util::stream::{Stream, StreamExt};
use http::Extensions;
use jsonrpsee_types::{ErrorObject, Id, InvalidRequestId, SubscriptionId};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{mpsc, oneshot};

/// Shared state whether a subscription has lagged or not.
#[derive(Debug, Clone)]
pub(crate) struct SubscriptionLagged(Arc<RwLock<bool>>);

/// Owned version of [`RawResponse`].
pub type RawResponseOwned = RawResponse<'static>;

impl SubscriptionLagged {
	/// Create a new [`SubscriptionLagged`].
	pub(crate) fn new() -> Self {
		Self(Arc::new(RwLock::new(false)))
	}

	/// A message has been missed.
	pub(crate) fn set_lagged(&self) {
		*self.0.write().expect("RwLock not poised; qed") = true;
	}

	/// Check whether the subscription has missed a message.
	pub(crate) fn has_lagged(&self) -> bool {
		*self.0.read().expect("RwLock not poised; qed")
	}
}

// Re-exports for the `rpc_params` macro.
#[doc(hidden)]
pub mod __reexports {
	// Needs to be in scope for `ArrayParams` to implement it.
	pub use crate::traits::ToRpcParams;
	// Main builder object for constructing the rpc parameters.
	pub use crate::params::ArrayParams;
}

/// [JSON-RPC](https://www.jsonrpc.org/specification) client interface that can make requests and notifications.
pub trait ClientT {
	/// Send a [notification request](https://www.jsonrpc.org/specification#notification)
	fn notification<Params>(&self, method: &str, params: Params) -> impl Future<Output = Result<(), Error>> + Send
	where
		Params: ToRpcParams + Send;

	/// Send a [method call request](https://www.jsonrpc.org/specification#request_object).
	fn request<R, Params>(&self, method: &str, params: Params) -> impl Future<Output = Result<R, Error>> + Send
	where
		R: DeserializeOwned,
		Params: ToRpcParams + Send;

	/// Send a [batch request](https://www.jsonrpc.org/specification#batch).
	///
	/// The response to batch are returned in the same order as it was inserted in the batch.
	///
	///
	/// Returns `Ok` if all requests in the batch were answered.
	/// Returns `Error` if the network failed or any of the responses could be parsed a valid JSON-RPC response.
	fn batch_request<'a, R>(
		&self,
		batch: BatchRequestBuilder<'a>,
	) -> impl Future<Output = Result<BatchResponse<'a, R>, Error>> + Send
	where
		R: DeserializeOwned + fmt::Debug + 'a;
}

/// [JSON-RPC](https://www.jsonrpc.org/specification) client interface that can make requests, notifications and subscriptions.
pub trait SubscriptionClientT: ClientT {
	/// Initiate a subscription by performing a JSON-RPC method call where the server responds with
	/// a `Subscription ID` that is used to fetch messages on that subscription,
	///
	/// The `subscribe_method` and `params` are used to ask for the subscription towards the
	/// server.
	///
	/// The params may be used as input for the subscription for the server to process.
	///
	/// The `unsubscribe_method` is used to close the subscription
	///
	/// The `Notif` param is a generic type to receive generic subscriptions, see [`Subscription`] for further
	/// documentation.
	fn subscribe<'a, Notif, Params>(
		&self,
		subscribe_method: &'a str,
		params: Params,
		unsubscribe_method: &'a str,
	) -> impl Future<Output = Result<Subscription<Notif>, Error>> + Send
	where
		Params: ToRpcParams + Send,
		Notif: DeserializeOwned;

	/// Register a method subscription, this is used to filter only server notifications that a user is interested in.
	///
	/// The `Notif` param is a generic type to receive generic subscriptions, see [`Subscription`] for further
	/// documentation.
	fn subscribe_to_method<Notif>(
		&self,
		method: &str,
	) -> impl Future<Output = Result<Subscription<Notif>, Error>> + Send
	where
		Notif: DeserializeOwned;
}

/// Marker trait to determine whether a type implements `Send` or not.
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}

/// Marker trait to determine whether a type implements `Send` or not.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}

#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}

#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// Transport interface to send data asynchronous.
pub trait TransportSenderT: 'static {
	/// Error that may occur during sending a message.
	type Error: std::error::Error + Send + Sync;

	/// Send.
	fn send(&mut self, msg: String) -> impl Future<Output = Result<(), Self::Error>> + MaybeSend;

	/// This is optional because it's most likely relevant for WebSocket transports only.
	/// You should only implement this is your transport supports sending periodic pings.
	///
	/// Send ping frame (opcode of 0x9).
	fn send_ping(&mut self) -> impl Future<Output = Result<(), Self::Error>> + MaybeSend {
		async { Ok(()) }
	}

	/// This is optional because it's most likely relevant for WebSocket transports only.
	/// You should only implement this is your transport supports being closed.
	///
	/// Send customized close message.
	fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + MaybeSend {
		async { Ok(()) }
	}
}

/// Message type received from the RPC server.
/// It can either be plain text data, bytes, or `Pong` messages.
#[derive(Debug, Clone)]
pub enum ReceivedMessage {
	/// Incoming packet contains plain `String` data.
	Text(String),
	/// Incoming packet contains bytes.
	Bytes(Vec<u8>),
	/// Incoming `Pong` frame as a reply to a previously submitted `Ping` frame.
	Pong,
}

/// Transport interface to receive data asynchronous.
pub trait TransportReceiverT: 'static {
	/// Error that may occur during receiving a message.
	type Error: std::error::Error + Send + Sync;

	/// Receive.
	fn receive(&mut self) -> impl Future<Output = Result<ReceivedMessage, Self::Error>> + MaybeSend;
}

/// Convert the given values to a [`crate::params::ArrayParams`] as expected by a
/// jsonrpsee Client (http or websocket).
///
/// # Panics
///
/// Panics if the serialization of parameters fails.
#[macro_export]
macro_rules! rpc_params {
	($($param:expr),*) => {
		{
			let mut params = $crate::client::__reexports::ArrayParams::new();
			$(
				if let Err(err) = params.insert($param) {
					panic!("Parameter `{}` cannot be serialized: {:?}", stringify!($param), err);
				}
			)*
			params
		}
	};
}

/// Subscription kind
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SubscriptionKind {
	/// Get notifications based on Subscription ID.
	Subscription(SubscriptionId<'static>),
	/// Get notifications based on method name.
	Method(String),
}

/// The reason why the subscription was closed.
#[derive(Debug, Copy, Clone)]
pub enum SubscriptionCloseReason {
	/// The connection was closed.
	ConnectionClosed,
	/// The subscription could not keep up with the server.
	Lagged,
}

/// Represent a client-side subscription which is implemented on top of
/// a bounded channel where it's possible that the receiver may
/// not keep up with the sender side a.k.a "slow receiver problem"
///
/// The Subscription will try to `unsubscribe` in the drop implementation
/// but it may fail if the underlying buffer is full.
/// Thus, if you want to ensure it's actually unsubscribed then
/// [`Subscription::unsubscribe`] is recommended to use.
///
/// ## Lagging
///
/// All messages from the server must be kept in a buffer in the client
/// until they are read by polling the [`Subscription`]. If you don't
/// poll the client subscription quickly enough, the buffer may fill
/// up and when subscription is full the subscription is then closed.
///
/// You can call [`Subscription::close_reason`] to determine why
/// the subscription was closed.
#[derive(Debug)]
pub struct Subscription<Notif> {
	is_closed: bool,
	/// Channel to send requests to the background task.
	to_back: mpsc::Sender<FrontToBack>,
	/// Channel from which we receive notifications from the server, as encoded JSON.
	rx: SubscriptionReceiver,
	/// Callback kind.
	kind: Option<SubscriptionKind>,
	/// Marker in order to pin the `Notif` parameter.
	marker: PhantomData<Notif>,
}

// `Subscription` does not automatically implement this due to `PhantomData<Notif>`,
// but type type has no need to be pinned.
impl<Notif> std::marker::Unpin for Subscription<Notif> {}

impl<Notif> Subscription<Notif> {
	/// Create a new subscription.
	fn new(to_back: mpsc::Sender<FrontToBack>, rx: SubscriptionReceiver, kind: SubscriptionKind) -> Self {
		Self { to_back, rx, kind: Some(kind), marker: PhantomData, is_closed: false }
	}

	/// Return the subscription type and, if applicable, ID.
	pub fn kind(&self) -> &SubscriptionKind {
		self.kind.as_ref().expect("only None after unsubscribe; qed")
	}

	/// Unsubscribe and consume the subscription.
	pub async fn unsubscribe(mut self) -> Result<(), Error> {
		let msg = match self.kind.take().expect("only None after unsubscribe; qed") {
			SubscriptionKind::Method(notif) => FrontToBack::UnregisterNotification(notif),
			SubscriptionKind::Subscription(sub_id) => FrontToBack::SubscriptionClosed(sub_id),
		};
		// If this fails the connection was already closed i.e, already "unsubscribed".
		let _ = self.to_back.send(msg).await;

		// wait until notif channel is closed then the subscription was closed.
		while self.rx.next().await.is_some() {}

		Ok(())
	}

	/// The reason why the subscription was closed.
	///
	/// Returns Some(reason) is the subscription was closed otherwise
	/// None is returned.
	pub fn close_reason(&self) -> Option<SubscriptionCloseReason> {
		let lagged = self.rx.lagged.has_lagged();

		// `is_closed` is only set if the subscription has been polled
		// and that is why lagged is checked here as well.
		if !self.is_closed && !lagged {
			return None;
		}

		if lagged { Some(SubscriptionCloseReason::Lagged) } else { Some(SubscriptionCloseReason::ConnectionClosed) }
	}
}

/// Batch request message.
#[derive(Debug)]
struct BatchMessage {
	/// Serialized batch request.
	raw: String,
	/// Request IDs.
	ids: Range<u64>,
	/// One-shot channel over which we send back the result of this request.
	send_back: oneshot::Sender<Result<Vec<RawResponseOwned>, InvalidRequestId>>,
}

/// Request message.
#[derive(Debug)]
struct RequestMessage {
	/// Serialized message.
	raw: String,
	/// Request ID.
	id: Id<'static>,
	/// One-shot channel over which we send back the result of this request.
	send_back: Option<oneshot::Sender<Result<RawResponseOwned, InvalidRequestId>>>,
}

/// Subscription message.
#[derive(Debug)]
struct SubscriptionMessage {
	/// Serialized message.
	raw: String,
	/// Request ID of the subscribe message.
	subscribe_id: Id<'static>,
	/// Request ID of the unsubscribe message.
	unsubscribe_id: Id<'static>,
	/// Method to use to unsubscribe later. Used if the channel unexpectedly closes.
	unsubscribe_method: String,
	/// If the subscription succeeds, we return a [`mpsc::Receiver`] that will receive notifications.
	/// When we get a response from the server about that subscription, we send the result over
	/// this channel.
	send_back: oneshot::Sender<Result<(SubscriptionReceiver, SubscriptionId<'static>), Error>>,
}

/// RegisterNotification message.
#[derive(Debug)]
struct RegisterNotificationMessage {
	/// Method name this notification handler is attached to
	method: String,
	/// We return a [`mpsc::Receiver`] that will receive notifications.
	/// When we get a response from the server about that subscription, we send the result over
	/// this channel.
	send_back: oneshot::Sender<Result<(SubscriptionReceiver, String), Error>>,
}

/// Message that the Client can send to the background task.
#[derive(Debug)]
enum FrontToBack {
	/// Send a batch request to the server.
	Batch(BatchMessage),
	/// Send a notification to the server.
	Notification(String),
	/// Send a request to the server.
	Request(RequestMessage),
	/// Send a subscription request to the server.
	Subscribe(SubscriptionMessage),
	/// Register a notification handler
	RegisterNotification(RegisterNotificationMessage),
	/// Unregister a notification handler
	UnregisterNotification(String),
	/// When a subscription channel is closed, we send this message to the background
	/// task to mark it ready for garbage collection.
	// NOTE: It is not possible to cancel pending subscriptions or pending requests.
	// Such operations will be blocked until a response is received or the background
	// thread has been terminated.
	SubscriptionClosed(SubscriptionId<'static>),
}

impl<Notif> Subscription<Notif>
where
	Notif: DeserializeOwned,
{
	/// Returns the next notification from the stream.
	/// This may return `None` if the subscription has been terminated,
	/// which may happen if the channel becomes full or is dropped.
	///
	/// **Note:** This has an identical signature to the [`StreamExt::next`]
	/// method (and delegates to that). Import [`StreamExt`] if you'd like
	/// access to other stream combinator methods.
	#[allow(clippy::should_implement_trait)]
	pub async fn next(&mut self) -> Option<Result<Notif, serde_json::Error>> {
		StreamExt::next(self).await
	}
}

impl<Notif> Stream for Subscription<Notif>
where
	Notif: DeserializeOwned,
{
	type Item = Result<Notif, serde_json::Error>;
	fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Option<Self::Item>> {
		let res = match futures_util::ready!(self.rx.poll_next_unpin(cx)) {
			Some(v) => Some(serde_json::from_str::<Notif>(v.get())),
			None => {
				self.is_closed = true;
				None
			}
		};

		Poll::Ready(res)
	}
}

impl<Notif> Drop for Subscription<Notif> {
	fn drop(&mut self) {
		// We can't actually guarantee that this goes through. If the background task is busy, then
		// the channel's buffer will be full.
		// However, when a notification arrives, the background task will realize that the channel
		// to the `Callback` has been closed.

		let msg = match self.kind.take() {
			Some(SubscriptionKind::Method(notif)) => FrontToBack::UnregisterNotification(notif),
			Some(SubscriptionKind::Subscription(sub_id)) => FrontToBack::SubscriptionClosed(sub_id),
			None => return,
		};
		let _ = self.to_back.try_send(msg);
	}
}

#[derive(Debug)]
/// Keep track of request IDs.
pub struct RequestIdManager {
	/// Get the next request ID.
	current_id: CurrentId,
	/// Request ID type.
	id_kind: IdKind,
}

impl RequestIdManager {
	/// Create a new `RequestIdGuard` with the provided concurrency limit.
	pub fn new(id_kind: IdKind) -> Self {
		Self { current_id: CurrentId::new(), id_kind }
	}

	/// Attempts to get the next request ID.
	pub fn next_request_id(&self) -> Id<'static> {
		self.id_kind.into_id(self.current_id.next())
	}

	/// Get a handle to the `IdKind`.
	pub fn as_id_kind(&self) -> IdKind {
		self.id_kind
	}
}

/// JSON-RPC request object id data type.
#[derive(Debug, Copy, Clone)]
pub enum IdKind {
	/// String.
	String,
	/// Number.
	Number,
}

impl IdKind {
	/// Generate an `Id` from number.
	pub fn into_id(self, id: u64) -> Id<'static> {
		match self {
			IdKind::Number => Id::Number(id),
			IdKind::String => Id::Str(format!("{id}").into()),
		}
	}
}

#[derive(Debug)]
struct CurrentId(AtomicUsize);

impl CurrentId {
	fn new() -> Self {
		CurrentId(AtomicUsize::new(0))
	}

	fn next(&self) -> u64 {
		self.0
			.fetch_add(1, Ordering::Relaxed)
			.try_into()
			.expect("usize -> u64 infallible, there are no CPUs > 64 bits; qed")
	}
}

/// Generate a range of IDs to be used in a batch request.
pub fn generate_batch_id_range(id: Id, len: u64) -> Result<Range<u64>, Error> {
	let id_start = id.try_parse_inner_as_number()?;
	let id_end = id_start
		.checked_add(len)
		.ok_or_else(|| Error::Custom("BatchID range wrapped; restart the client or try again later".to_string()))?;

	Ok(id_start..id_end)
}

/// Represent a single entry in a batch response.
pub type BatchEntry<'a, R> = Result<R, ErrorObject<'a>>;

/// Batch response.
#[derive(Debug, Clone)]
pub struct BatchResponse<'a, R> {
	successful_calls: usize,
	failed_calls: usize,
	responses: Vec<BatchEntry<'a, R>>,
}

impl<'a, R: fmt::Debug + 'a> BatchResponse<'a, R> {
	/// Create a new [`BatchResponse`].
	pub fn new(successful_calls: usize, responses: Vec<BatchEntry<'a, R>>, failed_calls: usize) -> Self {
		Self { successful_calls, responses, failed_calls }
	}

	/// Get the length of the batch response.
	pub fn len(&self) -> usize {
		self.responses.len()
	}

	/// Is empty.
	pub fn is_empty(&self) -> bool {
		self.responses.len() == 0
	}

	/// Get the number of successful calls in the batch.
	pub fn num_successful_calls(&self) -> usize {
		self.successful_calls
	}

	/// Get the number of failed calls in the batch.
	pub fn num_failed_calls(&self) -> usize {
		self.failed_calls
	}

	/// Returns `Ok(iterator)` if all responses were successful
	/// otherwise `Err(iterator)` is returned.
	///
	/// If you want get all responses if an error responses occurs use [`BatchResponse::into_iter`]
	/// instead where it's possible to implement customized logic.
	pub fn into_ok(
		self,
	) -> Result<impl Iterator<Item = R> + 'a + std::fmt::Debug, impl Iterator<Item = ErrorObject<'a>> + std::fmt::Debug>
	{
		if self.failed_calls > 0 {
			Err(self.into_iter().filter_map(|err| err.err()))
		} else {
			Ok(self.into_iter().filter_map(|r| r.ok()))
		}
	}

	/// Similar to [`BatchResponse::into_ok`] but takes the responses by reference instead.
	pub fn ok(
		&self,
	) -> Result<impl Iterator<Item = &R> + std::fmt::Debug, impl Iterator<Item = &ErrorObject<'a>> + std::fmt::Debug> {
		if self.failed_calls > 0 {
			Err(self.responses.iter().filter_map(|err| err.as_ref().err()))
		} else {
			Ok(self.responses.iter().filter_map(|r| r.as_ref().ok()))
		}
	}

	/// Returns an iterator over all responses.
	pub fn iter(&self) -> impl Iterator<Item = &BatchEntry<'_, R>> {
		self.responses.iter()
	}
}

impl<'a, R> IntoIterator for BatchResponse<'a, R> {
	type Item = BatchEntry<'a, R>;
	type IntoIter = std::vec::IntoIter<Self::Item>;

	fn into_iter(self) -> Self::IntoIter {
		self.responses.into_iter()
	}
}

#[derive(thiserror::Error, Debug)]
enum TrySubscriptionSendError {
	#[error("The subscription is closed")]
	Closed,
	#[error("A subscription message was dropped")]
	TooSlow(Box<RawValue>),
}

#[derive(Debug)]
pub(crate) struct SubscriptionSender {
	inner: mpsc::Sender<Box<RawValue>>,
	lagged: SubscriptionLagged,
}

impl SubscriptionSender {
	fn send(&self, msg: Box<RawValue>) -> Result<(), TrySubscriptionSendError> {
		match self.inner.try_send(msg) {
			Ok(_) => Ok(()),
			Err(TrySendError::Closed(_)) => Err(TrySubscriptionSendError::Closed),
			Err(TrySendError::Full(m)) => {
				self.lagged.set_lagged();
				Err(TrySubscriptionSendError::TooSlow(m))
			}
		}
	}
}

#[derive(Debug)]
pub(crate) struct SubscriptionReceiver {
	inner: mpsc::Receiver<Box<RawValue>>,
	lagged: SubscriptionLagged,
}

impl Stream for SubscriptionReceiver {
	type Item = Box<RawValue>;

	fn poll_next(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Option<Self::Item>> {
		self.inner.poll_recv(cx)
	}
}

fn subscription_channel(max_buf_size: usize) -> (SubscriptionSender, SubscriptionReceiver) {
	let (tx, rx) = mpsc::channel(max_buf_size);
	let lagged_tx = SubscriptionLagged::new();
	let lagged_rx = lagged_tx.clone();

	(SubscriptionSender { inner: tx, lagged: lagged_tx }, SubscriptionReceiver { inner: rx, lagged: lagged_rx })
}

/// Represents an active subscription returned by the server.
#[derive(Debug)]
pub struct SubscriptionResponse {
	/// The ID of the subscription.
	sub_id: SubscriptionId<'static>,
	// The receiver is used to receive notifications from the server and shouldn't be exposed to the user
	// from the middleware.
	stream: SubscriptionReceiver,
}

impl SubscriptionResponse {
	/// Get the subscription ID.
	pub fn subscription_id(&self) -> &SubscriptionId<'static> {
		&self.sub_id
	}
}

/// A raw JSON-RPC response object which can be either a JSON-RPC success or error response.
///
/// This is a wrapper around the `jsonrpsee_types::Response` type for ease of use
/// for middleware client implementations.
#[derive(Debug)]
pub struct RawResponse<'a>(jsonrpsee_types::Response<'a, Box<RawValue>>);

impl Serialize for RawResponse<'_> {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		self.0.serialize(serializer)
	}
}

impl<'a> From<jsonrpsee_types::Response<'a, Box<RawValue>>> for RawResponse<'a> {
	fn from(r: jsonrpsee_types::Response<'a, Box<RawValue>>) -> Self {
		Self(r)
	}
}

impl<'a> RawResponse<'a> {
	/// Whether this response is successful JSON-RPC response.
	pub fn is_success(&self) -> bool {
		match self.0.payload {
			jsonrpsee_types::ResponsePayload::Success(_) => true,
			jsonrpsee_types::ResponsePayload::Error(_) => false,
		}
	}

	/// Extract the error object from the response if it is an error.
	pub fn as_error(&self) -> Option<&ErrorObject<'_>> {
		match self.0.payload {
			jsonrpsee_types::ResponsePayload::Error(ref err) => Some(err),
			_ => None,
		}
	}

	// Extract the result field the response if it is a success.
	///
	/// Omits JSON-RPC specific fields like `jsonrpc` and `id`.
	pub fn as_success(&self) -> Option<&RawValue> {
		match self.0.payload {
			jsonrpsee_types::ResponsePayload::Success(ref res) => Some(res),
			_ => None,
		}
	}

	/// Get the request ID.
	pub fn id(&self) -> &Id<'a> {
		&self.0.id
	}

	/// Consume the response and extract the inner value.
	pub fn into_inner(self) -> jsonrpsee_types::Response<'a, Box<RawValue>> {
		self.0
	}

	/// Convert the response into an owned version.
	pub fn into_owned(self) -> RawResponseOwned {
		RawResponse(self.0.into_owned())
	}
}

impl ToJson for RawResponse<'_> {
	fn to_json(&self) -> Result<Box<RawValue>, serde_json::Error> {
		serde_json::value::to_raw_value(&self.0)
	}
}

/// Middleware batch response.
pub type MiddlewareBatchResponse = Vec<RawResponseOwned>;

/// Middleware method response which can be either a method call or a subscription response.
/// and treated differently.
#[derive(Debug)]
pub struct MiddlewareMethodResponse {
	rp: RawResponseOwned,
	subscription: Option<SubscriptionResponse>,
}

impl Serialize for MiddlewareMethodResponse {
	fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
	where
		S: serde::Serializer,
	{
		self.rp.serialize(serializer)
	}
}

impl Deref for MiddlewareMethodResponse {
	type Target = RawResponseOwned;

	fn deref(&self) -> &Self::Target {
		&self.rp
	}
}

impl MiddlewareMethodResponse {
	/// Create a new [`MiddlewareMethodResponse`] which is plain response.
	pub fn response(rp: RawResponseOwned) -> Self {
		Self { rp, subscription: None }
	}

	/// Create a new [`MiddlewareMethodResponse`] which is a subscription response.
	pub fn subscription_response(rp: RawResponseOwned, subscription: SubscriptionResponse) -> Self {
		Self { rp, subscription: Some(subscription) }
	}

	/// Convert the response into parts.
	pub fn into_parts(self) -> (RawResponseOwned, Option<SubscriptionResponse>) {
		(self.rp, self.subscription)
	}

	/// Extract the response.
	pub fn into_response(self) -> RawResponseOwned {
		self.rp
	}

	/// Extract the subscription response if it is a subscription.
	pub fn into_subscription(self) -> Option<SubscriptionResponse> {
		self.subscription
	}
}

/// Middleware notification response.
#[derive(Debug, Clone)]
pub struct MiddlewareNotifResponse(Extensions);

impl From<Extensions> for MiddlewareNotifResponse {
	fn from(extensions: Extensions) -> Self {
		Self(extensions)
	}
}

impl MiddlewareNotifResponse {
	/// Get the extensions.
	pub fn extensions(&self) -> &Extensions {
		&self.0
	}

	/// Get the extensions mutable.
	pub fn extensions_mut(&mut self) -> &mut Extensions {
		&mut self.0
	}
}

impl<T: Serialize> ToJson for Result<T, Error> {
	fn to_json(&self) -> Result<Box<RawValue>, serde_json::Error> {
		match self {
			Ok(v) => serde_json::value::to_raw_value(v),
			Err(e) => serde_json::value::to_raw_value(&e.to_string()),
		}
	}
}
