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

use super::{helpers::MethodSink, MethodResponse, ResponsePayload};
use crate::{
    error::SubscriptionError,
    server::{
        error::{DisconnectError, PendingSubscriptionAcceptError, SendTimeoutError, TrySendError},
        rpc_module::ConnectionId,
    },
    traits::IdProvider,
};
use jsonrpsee_types::{
    response::SubscriptionPayloadError, ErrorCode, ErrorObjectOwned, Id, SubscriptionId,
    SubscriptionPayload, SubscriptionResponse,
};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use serde::Serialize;
use serde_json::value::RawValue;
use std::{collections::hash_map::Entry, sync::Arc, time::Duration};
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};

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
    Complete(String),
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
    pub fn new(
        method: &str,
        subscription: SubscriptionId,
        result: &impl Serialize,
    ) -> Result<Self, serde_json::Error> {
        let json = serde_json::to_string(&SubscriptionResponse::new(
            method.into(),
            SubscriptionPayload { subscription, result },
        ))?;
        Ok(Self::from_complete_message(json))
    }

    pub(crate) fn from_complete_message(msg: String) -> Self {
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
// NOTE: The reason why we use `mpsc` here is because it allows `IsUnsubscribed::unsubscribed`
// to be &self instead of &mut self.
#[derive(Debug, Clone)]
pub struct IsUnsubscribed(mpsc::Sender<()>);

impl IsUnsubscribed {
    /// Returns true if the unsubscribe method has been invoked or the subscription has been
    /// canceled.
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
/// If this is dropped without calling `PendingSubscription::reject` or
/// `PendingSubscriptionSink::accept` a default error is sent out as response to the subscription
/// call.
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
        let (tx, rx) = mpsc::channel(1);

        // The key is reserved before the acknowledgement is sent: an occupied key belongs to a live
        // subscription of this connection, which must not be replaced.
        let reserved = match self.subscribers.lock().entry(self.uniq_sub.clone()) {
            Entry::Vacant(entry) => {
                entry.insert((self.inner.clone(), rx));
                true
            }
            Entry::Occupied(_) => false,
        };

        if !reserved {
            self.reject(ErrorCode::InternalError).await;
            return Err(PendingSubscriptionAcceptError);
        }

        // Dropping the sink releases the reservation again if the acknowledgement isn't delivered.
        let sink = SubscriptionSink {
            inner: self.inner,
            method: self.method,
            subscribers: self.subscribers,
            uniq_sub: self.uniq_sub,
            unsubscribe: IsUnsubscribed(tx),
            _permit: Some(Arc::new(self.permit)),
        };

        let response = MethodResponse::subscription_response(
            self.id,
            ResponsePayload::success_borrowed(&sink.uniq_sub.sub_id),
            sink.inner.max_response_size() as usize,
        );
        let success = response.is_success();

        // TODO: #1052
        //
        // Ideally the message should be sent only once.
        //
        // The same message is sent twice here because one is sent directly to the transport layer
        // and the other one is sent internally to accept the subscription.
        sink.inner.send(response.to_json()).await.map_err(|_| PendingSubscriptionAcceptError)?;
        self.subscribe.send(response).map_err(|_| PendingSubscriptionAcceptError)?;

        if success {
            Ok(sink)
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

    /// Attempts to immediately send out the message as JSON string to the subscribers but fails if
    /// the channel is full or the connection/subscription is closed
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

/// This wraps [`tokio::sync::Semaphore`] and is used to limit the number of subscriptions per
/// connection.
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

pub(crate) fn sub_message_to_json(
    msg: SubscriptionMessage,
    sub_id: &SubscriptionId,
    method: &str,
) -> String {
    match msg.0 {
        SubscriptionMessageInner::Complete(msg) => msg,
        SubscriptionMessageInner::NeedsData(result) => {
            serde_json::to_string(&SubscriptionResponse::new(
                method.into(),
                SubscriptionPayload { subscription: sub_id.clone(), result },
            ))
            .expect("Serialize infallible; qed")
        }
    }
}

pub(crate) fn sub_err_to_json(
    error: SubscriptionError,
    sub_id: SubscriptionId,
    method: &str,
) -> String {
    serde_json::to_string(&jsonrpsee_types::response::SubscriptionError::new(
        method.into(),
        SubscriptionPayloadError { subscription: sub_id, error },
    ))
    .expect("Serialize infallible; qed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::{MethodCallback, Methods, RpcModule};
    use jsonrpsee_types::Params;
    use serde_json::value::to_raw_value;

    const SUB_ID: &str = "0xdeadbeef";

    /// Id provider that hands out the same id for every subscription.
    #[derive(Debug)]
    struct ConstantId;

    impl IdProvider for ConstantId {
        fn next_id(&self) -> SubscriptionId<'static> {
            SUB_ID.to_string().into()
        }
    }

    /// Drives the callbacks that `RpcModule::register_subscription_raw` registers.
    struct Harness {
        methods: Methods,
        pending: mpsc::UnboundedReceiver<PendingSubscriptionSink>,
        permits: BoundedSubscriptions,
    }

    impl Harness {
        fn new() -> Self {
            let (pending_tx, pending) = mpsc::unbounded_channel();
            let mut module = RpcModule::new(pending_tx);
            module
                .register_subscription_raw("sub", "notif", "unsub", |_, pending, ctx| {
                    ctx.send(pending).unwrap();
                })
                .unwrap();

            Self {
                methods: Methods::clone(&module),
                pending,
                permits: BoundedSubscriptions::new(16),
            }
        }

        /// Calls the subscribe method with request id `id` and returns the pending subscription
        /// together with the future that resolves to the answer of the subscribe call.
        fn subscribe(
            &mut self,
            id: u64,
            conn_id: ConnectionId,
            sink: &mpsc::Sender<String>,
        ) -> (PendingSubscriptionSink, futures_util::future::BoxFuture<'static, MethodResponse>)
        {
            let state = SubscriptionState {
                conn_id,
                id_provider: &ConstantId,
                subscription_permit: self.permits.acquire().unwrap(),
            };
            let Some(MethodCallback::Subscription(subscribe)) = self.methods.method("sub") else {
                panic!("`sub` is a subscription");
            };
            let response =
                subscribe(Id::Number(id), Params::new(None), MethodSink::new(sink.clone()), state);
            (self.pending.try_recv().unwrap(), response)
        }

        /// Calls the unsubscribe method for `SUB_ID` and returns its json answer.
        fn unsubscribe(&self, id: u64, conn_id: ConnectionId) -> String {
            let params = format!(r#"["{SUB_ID}"]"#);
            let Some(MethodCallback::Unsubscription(unsubscribe)) = self.methods.method("unsub")
            else {
                panic!("`unsub` is an unsubscription");
            };
            unsubscribe(Id::Number(id), Params::new(Some(&params)), conn_id, usize::MAX).into_json()
        }
    }

    #[tokio::test]
    async fn colliding_subscription_id_keeps_existing_subscription() {
        let mut harness = Harness::new();
        let conn_id = ConnectionId(0);
        let (tx, mut rx) = mpsc::channel(8);

        let (pending, response) = harness.subscribe(1, conn_id, &tx);
        let first = pending.accept().await.unwrap();
        let ack = r#"{"jsonrpc":"2.0","id":1,"result":"0xdeadbeef"}"#;
        assert_eq!(response.await.into_json(), ack);
        assert_eq!(rx.recv().await.unwrap(), ack);

        // The provider hands out the id again: the second call fails with exactly one error.
        let (pending, response) = harness.subscribe(2, conn_id, &tx);
        assert!(matches!(pending.accept().await, Err(PendingSubscriptionAcceptError)));
        let error =
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32603,"message":"Internal error"}}"#;
        assert_eq!(response.await.into_json(), error);
        assert_eq!(rx.recv().await.unwrap(), error);
        assert!(rx.try_recv().is_err());

        // The first subscription is untouched and still delivers items.
        assert!(!first.is_closed());
        first.send(to_raw_value(&"item").unwrap()).await.unwrap();
        assert_eq!(
            rx.recv().await.unwrap(),
            r#"{"jsonrpc":"2.0","method":"notif","params":{"subscription":"0xdeadbeef","result":"item"}}"#
        );

        // It can be unsubscribed, which removes it.
        assert_eq!(harness.unsubscribe(3, conn_id), r#"{"jsonrpc":"2.0","id":3,"result":true}"#);
        assert!(first.is_closed());
        assert_eq!(harness.unsubscribe(4, conn_id), r#"{"jsonrpc":"2.0","id":4,"result":false}"#);
    }

    #[tokio::test]
    async fn same_subscription_id_on_different_connections() {
        let mut harness = Harness::new();
        let (tx, _rx) = mpsc::channel(8);

        let (pending, _first_response) = harness.subscribe(1, ConnectionId(0), &tx);
        let first = pending.accept().await.unwrap();
        let (pending, _second_response) = harness.subscribe(1, ConnectionId(1), &tx);
        let second = pending.accept().await.unwrap();

        assert_eq!(
            harness.unsubscribe(2, ConnectionId(0)),
            r#"{"jsonrpc":"2.0","id":2,"result":true}"#
        );
        assert!(first.is_closed());
        assert!(!second.is_closed());
        assert_eq!(
            harness.unsubscribe(3, ConnectionId(1)),
            r#"{"jsonrpc":"2.0","id":3,"result":true}"#
        );
        assert!(second.is_closed());
    }

    #[tokio::test]
    async fn failed_acknowledgement_releases_subscription_id() {
        let mut harness = Harness::new();
        let conn_id = ConnectionId(0);

        // The connection is gone, so the acknowledgement can't be delivered.
        let (tx, rx) = mpsc::channel(8);
        drop(rx);
        let (pending, _) = harness.subscribe(1, conn_id, &tx);
        assert!(matches!(pending.accept().await, Err(PendingSubscriptionAcceptError)));
        assert_eq!(harness.unsubscribe(2, conn_id), r#"{"jsonrpc":"2.0","id":2,"result":false}"#);

        // The id isn't blocked for later subscriptions.
        let (tx, _rx) = mpsc::channel(8);
        let (pending, _response) = harness.subscribe(3, conn_id, &tx);
        let sink = pending.accept().await.unwrap();
        assert_eq!(harness.unsubscribe(4, conn_id), r#"{"jsonrpc":"2.0","id":4,"result":true}"#);
        assert!(sink.is_closed());
    }
}
