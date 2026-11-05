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

//! Subscription related types and traits for server implementations.

use super::helpers::MethodSink;
use super::{MethodResponse, MethodsError, ResponsePayload};
use crate::server::LOG_TARGET;
use crate::server::error::{DisconnectError, PendingSubscriptionAcceptError, SendTimeoutError, TrySendError};
use crate::server::rpc_module::ConnectionId;
use crate::{error::SubscriptionError, traits::IdProvider};
use jsonrpsee_types::SubscriptionPayload;
use jsonrpsee_types::response::SubscriptionPayloadError;
use jsonrpsee_types::{ErrorObjectOwned, Id, SubscriptionId, SubscriptionResponse};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::{sync::Arc, time::Duration};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};

/// Type-alias for subscribers.
pub type Subscribers = Arc<Mutex<FxHashMap<SubscriptionKey, (MethodSink, mpsc::Receiver<()>)>>>;
/// Subscription permit.
pub type SubscriptionPermit = OwnedSemaphorePermit;

/// Convert something into a subscription close notification
/// before a subscription is terminated.
pub trait IntoSubscriptionCloseResponse {
	/// Convert something into a subscription response
	fn into_response(self) -> SubscriptionCloseResponse;
}

/// Represents what action that will sent when a subscription callback returns.
#[derive(Debug)]
pub enum SubscriptionCloseResponse {
	/// No further message will be sent.
	None,
	/// Send a subscription notification.
	///
	/// The subscription notification has the following format:
	///
	/// ```json
	/// {
	///  "jsonrpc": "2.0",
	///  "method": "<method>",
	///  "params": {
	///    "subscription": "<subscriptionID>",
	///    "result": <your msg>
	///    }
	///  }
	/// }
	/// ```
	Notif(SubscriptionMessage),
	/// Send a subscription error notification
	///
	/// The error notification has the following format:
	///
	/// ```json
	/// {
	///  "jsonrpc": "2.0",
	///  "method": "<method>",
	///  "params": {
	///    "subscription": "<subscriptionID>",
	///    "error": <your msg>
	///    }
	///  }
	/// }
	/// ```
	NotifErr(SubscriptionError),
}

impl IntoSubscriptionCloseResponse for Result<(), SubscriptionError> {
	fn into_response(self) -> SubscriptionCloseResponse {
		match self {
			Ok(()) => SubscriptionCloseResponse::None,
			Err(e) => SubscriptionCloseResponse::NotifErr(e),
		}
	}
}

impl IntoSubscriptionCloseResponse for () {
	fn into_response(self) -> SubscriptionCloseResponse {
		SubscriptionCloseResponse::None
	}
}

impl IntoSubscriptionCloseResponse for SubscriptionCloseResponse {
	fn into_response(self) -> Self {
		self
	}
}

/// A complete subscription message or partial subscription message.
#[derive(Debug, Clone)]
pub enum SubscriptionMessageInner {
	/// Complete JSON message.
	Complete(Box<RawValue>),
	/// Need subscription ID and method name.
	NeedsData(Box<RawValue>),
}

/// Subscription message.
#[derive(Debug, Clone)]
pub struct SubscriptionMessage(pub(crate) SubscriptionMessageInner);

impl From<Box<RawValue>> for SubscriptionMessage {
	fn from(json: Box<RawValue>) -> Self {
		Self(SubscriptionMessageInner::NeedsData(json))
	}
}

impl SubscriptionMessage {
	/// Create a subscription message this is more efficient than [`SubscriptionMessage::from`]
	/// because it only allocates once.
	///
	/// Fails if the json `result` couldn't be serialized.
	pub fn new(method: &str, subscription: SubscriptionId, result: &impl Serialize) -> Result<Self, serde_json::Error> {
		let json = serde_json::value::to_raw_value(&SubscriptionResponse::new(
			method.into(),
			SubscriptionPayload { subscription, result },
		))?;
		Ok(Self::from_complete_message(json))
	}

	pub(crate) fn from_complete_message(msg: Box<RawValue>) -> Self {
		SubscriptionMessage(SubscriptionMessageInner::Complete(msg))
	}
}

/// Represent a unique subscription entry based on [`SubscriptionId`] and [`ConnectionId`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SubscriptionKey {
	pub(crate) conn_id: ConnectionId,
	pub(crate) sub_id: SubscriptionId<'static>,
}

/// Represents a subscription until it is unsubscribed.
///
// NOTE: The reason why we use `mpsc` here is because it allows `IsUnsubscribed::unsubscribed`
// to be &self instead of &mut self.
#[derive(Debug, Clone)]
pub struct IsUnsubscribed(mpsc::Sender<()>);

impl IsUnsubscribed {
	/// Returns true if the unsubscribe method has been invoked or the subscription has been canceled.
	///
	/// This can be called multiple times as the element in the channel is never
	/// removed.
	pub fn is_unsubscribed(&self) -> bool {
		self.0.is_closed()
	}

	/// Wrapper over [`tokio::sync::mpsc::Sender::closed`]
	///
	/// # Cancel safety
	///
	/// This method is cancel safe. Once the channel is closed,
	/// it stays closed forever and all future calls to closed will return immediately.
	pub async fn unsubscribed(&self) {
		self.0.closed().await;
	}
}

/// Represents a single subscription that is waiting to be accepted or rejected.
///
/// If this is dropped without calling `PendingSubscription::reject` or `PendingSubscriptionSink::accept`
/// a default error is sent out as response to the subscription call.
///
/// Thus, if you want a customized error message then `PendingSubscription::reject` must be called.
#[derive(Debug)]
#[must_use = "PendingSubscriptionSink does nothing unless `accept` or `reject` is called"]
pub struct PendingSubscriptionSink {
	/// Sink.
	pub(crate) inner: MethodSink,
	/// MethodCallback.
	pub(crate) method: &'static str,
	/// Shared Mutex of subscriptions for this method.
	pub(crate) subscribers: Subscribers,
	/// Unique subscription.
	pub(crate) uniq_sub: SubscriptionKey,
	/// ID of the `subscription call` (i.e. not the same as subscription id) which is used
	/// to reply to subscription method call and must only be used once.
	pub(crate) id: Id<'static>,
	/// Sender to answer the subscribe call.
	pub(crate) subscribe: oneshot::Sender<MethodResponse>,
	/// Subscription permit.
	pub(crate) permit: OwnedSemaphorePermit,
}

impl PendingSubscriptionSink {
	/// Reject the subscription by responding to the subscription method call with
	/// the error message from [`jsonrpsee_types::error::ErrorObject`].
	///
	/// # Note
	///
	/// If this is used in the async subscription callback
	/// the return value is simply ignored because no further notification are propagated
	/// once reject has been called.
	pub async fn reject(self, err: impl Into<ErrorObjectOwned>) {
		let err = MethodResponse::subscription_error(self.id, err.into());
		_ = self.inner.send(err.to_json()).await;
		_ = self.subscribe.send(err);
	}

	/// Attempt to accept the subscription and respond the subscription method call.
	///
	/// # Panics
	///
	/// Panics if the subscription response exceeded the `max_response_size`.
	pub async fn accept(self) -> Result<SubscriptionSink, PendingSubscriptionAcceptError> {
		let response = MethodResponse::subscription_response(
			self.id,
			ResponsePayload::success_borrowed(&self.uniq_sub.sub_id),
			self.inner.max_response_size() as usize,
		);
		let success = response.is_success();

		// TODO: #1052
		//
		// Ideally the message should be sent only once.
		//
		// The same message is sent twice here because one is sent directly to the transport layer and
		// the other one is sent internally to accept the subscription.
		self.inner.send(response.to_json()).await.map_err(|_| PendingSubscriptionAcceptError)?;
		self.subscribe.send(response).map_err(|_| PendingSubscriptionAcceptError)?;

		if success {
			let (tx, rx) = mpsc::channel(1);
			self.subscribers.lock().insert(self.uniq_sub.clone(), (self.inner.clone(), rx));
			Ok(SubscriptionSink {
				inner: self.inner,
				method: self.method,
				subscribers: self.subscribers,
				uniq_sub: self.uniq_sub,
				unsubscribe: IsUnsubscribed(tx),
				_permit: Some(Arc::new(self.permit)),
			})
		} else {
			panic!(
				"The subscription response was too big; adjust the `max_response_size` or change Subscription ID generation"
			);
		}
	}

	/// Get the subscription ID.
	pub fn subscription_id(&self) -> SubscriptionId<'static> {
		self.uniq_sub.sub_id.clone()
	}

	/// Returns connection identifier, which was used to perform pending subscription request
	pub fn connection_id(&self) -> ConnectionId {
		self.uniq_sub.conn_id
	}

	/// Get the capacity of the channel.
	pub fn capacity(&self) -> usize {
		self.inner.capacity()
	}

	/// Get the max capacity of the channel.
	pub fn max_capacity(&self) -> usize {
		self.inner.max_capacity()
	}

	/// Get the method name.
	pub fn method_name(&self) -> &str {
		self.method
	}
}

/// A handle to an active subscription.
///
/// Clones share the same subscription. The subscription is torn down when the last
/// clone is dropped or the client explicitly unsubscribes.
#[derive(Debug, Clone)]
pub struct SubscriptionSink {
	/// Sink.
	inner: MethodSink,
	/// MethodCallback.
	method: &'static str,
	/// Shared Mutex of subscriptions for this method.
	subscribers: Subscribers,
	/// Unique subscription.
	uniq_sub: SubscriptionKey,
	/// A future to that fires once the unsubscribe method has been called.
	unsubscribe: IsUnsubscribed,
	/// Subscription permit.
	_permit: Option<Arc<SubscriptionPermit>>,
}

impl SubscriptionSink {
	/// Get the subscription ID.
	pub fn subscription_id(&self) -> SubscriptionId<'static> {
		self.uniq_sub.sub_id.clone()
	}

	/// Get the method name.
	pub fn method_name(&self) -> &str {
		self.method
	}

	/// Get the connection ID.
	pub fn connection_id(&self) -> ConnectionId {
		self.uniq_sub.conn_id
	}

	/// Send out a response on the subscription and wait until there is capacity.
	///
	///
	/// Returns
	/// - `Ok(())` if the message could be sent.
	/// - `Err(unsent_msg)` if the connection or subscription was closed.
	///
	/// # Cancel safety
	///
	/// This method is cancel-safe and dropping a future loses its spot in the waiting queue.
	pub async fn send(&self, msg: impl Into<SubscriptionMessage>) -> Result<(), DisconnectError> {
		let msg = msg.into();

		// Only possible to trigger when the connection is dropped.
		if self.is_closed() {
			return Err(DisconnectError(msg));
		}

		let json = sub_message_to_json(msg, &self.uniq_sub.sub_id, self.method);
		self.inner.send(json).await
	}

	/// Similar to `SubscriptionSink::send` but only waits for a limited time.
	pub async fn send_timeout(
		&self,
		msg: impl Into<SubscriptionMessage>,
		timeout: Duration,
	) -> Result<(), SendTimeoutError> {
		let msg = msg.into();

		// Only possible to trigger when the connection is dropped.
		if self.is_closed() {
			return Err(SendTimeoutError::Closed(msg));
		}

		let json = sub_message_to_json(msg, &self.uniq_sub.sub_id, self.method);
		self.inner.send_timeout(json, timeout).await
	}

	/// Attempts to immediately send out the message as JSON string to the subscribers but fails if the
	/// channel is full or the connection/subscription is closed
	///
	///
	/// This differs from [`SubscriptionSink::send`] where it will until there is capacity
	/// in the channel.
	pub fn try_send(&mut self, msg: impl Into<SubscriptionMessage>) -> Result<(), TrySendError> {
		let msg = msg.into();

		// Only possible to trigger when the connection is dropped.
		if self.is_closed() {
			return Err(TrySendError::Closed(msg));
		}

		let json = sub_message_to_json(msg, &self.uniq_sub.sub_id, self.method);
		self.inner.try_send(json)
	}

	/// Returns whether the subscription is closed.
	pub fn is_closed(&self) -> bool {
		self.inner.is_closed() || !self.is_active_subscription()
	}

	/// Completes when the subscription has been closed.
	pub async fn closed(&self) {
		// Both are cancel-safe thus ok to use select here.
		tokio::select! {
			_ = self.inner.closed() => (),
			_ = self.unsubscribe.unsubscribed() => (),
		}
	}

	/// Get the capacity of the subscription.
	pub fn capacity(&self) -> usize {
		self.inner.capacity()
	}

	/// Get the max capacity of the subscription.
	pub fn max_capacity(&self) -> usize {
		self.inner.max_capacity()
	}

	fn is_active_subscription(&self) -> bool {
		!self.unsubscribe.is_unsubscribed()
	}
}

impl Drop for SubscriptionSink {
	fn drop(&mut self) {
		if self.is_active_subscription() {
			// Only the last clone should tear down the subscription (see #1622).
			// The lock serializes concurrent drops so that exactly one `try_unwrap` succeeds.
			let mut subs = self.subscribers.lock();
			if self._permit.take().is_some_and(|p| Arc::try_unwrap(p).is_ok()) {
				subs.remove(&self.uniq_sub);
			}
		}
	}
}

/// Wrapper struct that maintains a subscription "mainly" for testing.
#[derive(Debug)]
pub struct Subscription {
	pub(crate) rx: mpsc::Receiver<Box<RawValue>>,
	pub(crate) sub_id: SubscriptionId<'static>,
}

impl Subscription {
	/// Close the subscription channel.
	pub fn close(&mut self) {
		tracing::trace!(target: LOG_TARGET, "[Subscription::close] Notifying");
		self.rx.close();
	}

	/// Get the subscription ID
	pub fn subscription_id(&self) -> &SubscriptionId<'static> {
		&self.sub_id
	}

	/// Receives the next value on the subscription if the value could be decoded as T.
	pub async fn next<T: DeserializeOwned>(&mut self) -> Option<Result<(T, SubscriptionId<'static>), MethodsError>> {
		let raw = self.rx.recv().await?;

		tracing::debug!(target: LOG_TARGET, "[Subscription::next]: rx {}", raw);

		// clippy complains about this but it doesn't compile without the extra res binding.
		#[allow(clippy::let_and_return)]
		let res = match serde_json::from_str::<SubscriptionResponse<T>>(raw.get()) {
			Ok(r) => Some(Ok((r.params.result, r.params.subscription.into_owned()))),
			Err(e) => {
				match serde_json::from_str::<jsonrpsee_types::response::SubscriptionError<&RawValue>>(raw.get()) {
					Ok(_) => None,
					Err(_) => Some(Err(e.into())),
				}
			}
		};
		res
	}
}

impl Drop for Subscription {
	fn drop(&mut self) {
		self.close();
	}
}

/// This wraps [`tokio::sync::Semaphore`] and is used to limit the number of subscriptions per connection.
#[derive(Debug, Clone)]
pub struct BoundedSubscriptions {
	guard: Arc<Semaphore>,
	max: u32,
}

impl BoundedSubscriptions {
	/// Create a new bounded subscription.
	pub fn new(max_subscriptions: u32) -> Self {
		Self { guard: Arc::new(Semaphore::new(max_subscriptions as usize)), max: max_subscriptions }
	}

	/// Attempts to acquire a subscription slot.
	///
	/// Fails if `max_subscriptions` have been exceeded.
	pub fn acquire(&self) -> Option<SubscriptionPermit> {
		Arc::clone(&self.guard).try_acquire_owned().ok()
	}

	/// Get the maximum number of permitted subscriptions.
	pub const fn max(&self) -> u32 {
		self.max
	}
}

#[derive(Debug)]
/// Helper struct to manage subscriptions.
pub struct SubscriptionState<'a> {
	/// Connection ID
	pub conn_id: ConnectionId,
	/// ID provider.
	pub id_provider: &'a dyn IdProvider,
	/// Subscription limit
	pub subscription_permit: SubscriptionPermit,
}

pub(crate) fn sub_message_to_json(msg: SubscriptionMessage, sub_id: &SubscriptionId, method: &str) -> Box<RawValue> {
	match msg.0 {
		SubscriptionMessageInner::Complete(msg) => msg,
		SubscriptionMessageInner::NeedsData(result) => serde_json::value::to_raw_value(&SubscriptionResponse::new(
			method.into(),
			SubscriptionPayload { subscription: sub_id.clone(), result },
		))
		.expect("Serialize infallible; qed"),
	}
}

pub(crate) fn sub_err_to_json(error: SubscriptionError, sub_id: SubscriptionId, method: &str) -> Box<RawValue> {
	serde_json::value::to_raw_value(&jsonrpsee_types::response::SubscriptionError::new(
		method.into(),
		SubscriptionPayloadError { subscription: sub_id, error },
	))
	.expect("Serialize infallible; qed")
}
