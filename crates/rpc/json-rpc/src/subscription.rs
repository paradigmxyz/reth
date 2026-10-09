use crate::{
    reject_too_many_subscriptions, ConnectionId, ErrorObject, Id, MethodResponse, SubscriptionId,
};
use rand::{distr::Alphanumeric, Rng};
use rustc_hash::FxHashMap;
use serde::Serialize;
use std::{
    fmt,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot, watch, OwnedSemaphorePermit, Semaphore};

/// Result of a subscription method.
pub type SubscriptionResult = Result<(), StringError>;

/// Error type of [`SubscriptionResult`], convertible from anything that implements
/// [`ToString`].
#[derive(Debug)]
pub struct StringError(String);

impl StringError {
    /// Returns the error message.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<T: ToString> From<T> for StringError {
    fn from(value: T) -> Self {
        Self(value.to_string())
    }
}

/// Converts the output of a subscription callback into a [`SubscriptionResult`].
pub trait IntoSubscriptionResult {
    /// Converts `self` into a [`SubscriptionResult`].
    fn into_subscription_result(self) -> SubscriptionResult;
}

impl IntoSubscriptionResult for () {
    fn into_subscription_result(self) -> SubscriptionResult {
        Ok(())
    }
}

impl IntoSubscriptionResult for SubscriptionResult {
    fn into_subscription_result(self) -> SubscriptionResult {
        self
    }
}

/// Generates subscription ids.
pub trait IdProvider: Send + Sync + fmt::Debug {
    /// Returns the next subscription id.
    fn next_id(&self) -> SubscriptionId;
}

impl<T: IdProvider + ?Sized> IdProvider for Box<T> {
    fn next_id(&self) -> SubscriptionId {
        (**self).next_id()
    }
}

/// Generates random integer subscription ids.
#[derive(Clone, Copy, Debug, Default)]
pub struct RandomIntegerIdProvider;

impl IdProvider for RandomIntegerIdProvider {
    fn next_id(&self) -> SubscriptionId {
        // Keep ids within the integer range JavaScript can represent exactly.
        SubscriptionId::Num(rand::random::<u64>() & ((1 << 53) - 1))
    }
}

/// Generates random alphanumeric subscription ids.
#[derive(Clone, Copy, Debug)]
pub struct RandomStringIdProvider {
    len: usize,
}

impl RandomStringIdProvider {
    /// Creates a new provider of ids with `len` characters.
    pub const fn new(len: usize) -> Self {
        Self { len }
    }
}

impl IdProvider for RandomStringIdProvider {
    fn next_id(&self) -> SubscriptionId {
        let id = rand::rng().sample_iter(Alphanumeric).take(self.len).map(char::from).collect();
        SubscriptionId::Str(id)
    }
}

/// Subscription state of a connection.
#[derive(Debug)]
pub(crate) struct Connection {
    id: ConnectionId,
    tx: mpsc::Sender<String>,
    /// Active subscriptions by id, with their unsubscribe method.
    ///
    /// Dropping an entry closes the subscription.
    subscriptions: Mutex<FxHashMap<SubscriptionId, (&'static str, watch::Receiver<()>)>>,
    permits: Arc<Semaphore>,
    max_subscriptions: u32,
    id_provider: Arc<dyn IdProvider>,
}

impl Connection {
    pub(crate) fn new(
        id: ConnectionId,
        tx: mpsc::Sender<String>,
        max_subscriptions: u32,
        id_provider: Arc<dyn IdProvider>,
    ) -> Self {
        Self {
            id,
            tx,
            subscriptions: Default::default(),
            permits: Arc::new(Semaphore::new(max_subscriptions as usize)),
            max_subscriptions,
            id_provider,
        }
    }

    /// Creates a pending subscription, or returns an error if the connection has too many.
    pub(crate) fn subscribe(
        self: &Arc<Self>,
        id: Id,
        method: &'static str,
        unsubscribe: &'static str,
    ) -> Result<(PendingSubscriptionSink, oneshot::Receiver<MethodResponse>), ErrorObject> {
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| reject_too_many_subscriptions(self.max_subscriptions as usize))?;
        let (respond, rx) = oneshot::channel();
        let sink = PendingSubscriptionSink {
            id,
            sub_id: self.id_provider.next_id(),
            method,
            unsubscribe,
            conn: self.clone(),
            respond,
            permit,
        };
        Ok((sink, rx))
    }

    /// Closes the subscription, returning `true` if it was active.
    pub(crate) fn unsubscribe(&self, unsubscribe: &str, sub_id: &SubscriptionId) -> bool {
        let mut subscriptions = self.subscriptions.lock().unwrap();
        if subscriptions.get(sub_id).is_some_and(|(method, _)| *method == unsubscribe) {
            subscriptions.remove(sub_id);
            true
        } else {
            false
        }
    }
}

/// A subscription that was not yet accepted or rejected.
///
/// Dropping it rejects the subscription with an internal error.
#[derive(Debug)]
pub struct PendingSubscriptionSink {
    id: Id,
    sub_id: SubscriptionId,
    method: &'static str,
    unsubscribe: &'static str,
    conn: Arc<Connection>,
    respond: oneshot::Sender<MethodResponse>,
    permit: OwnedSemaphorePermit,
}

impl PendingSubscriptionSink {
    /// Accepts the subscription and sends the subscription id to the client.
    ///
    /// Resolves once the response was queued on the connection, so that it precedes all
    /// notifications.
    pub async fn accept(self) -> Result<SubscriptionSink, DisconnectError> {
        let Self { id, sub_id, method, unsubscribe, conn, respond, permit } = self;
        let (on_sent, sent) = oneshot::channel();
        let response = MethodResponse::response(id, &sub_id, usize::MAX).with_on_sent(on_sent);

        // Register before answering so an immediate unsubscribe finds the subscription.
        let (close, close_rx) = watch::channel(());
        conn.subscriptions.lock().unwrap().insert(sub_id.clone(), (unsubscribe, close_rx));
        let sink = SubscriptionSink { method, sub_id, close, conn, _permit: permit };

        // The response uses the slot the connection reserved for the call, so it never waits for
        // capacity held by other pending subscriptions.
        respond.send(response).map_err(|_| DisconnectError)?;
        sent.await.map_err(|_| DisconnectError)?;
        Ok(sink)
    }

    /// Rejects the subscription with the given error.
    pub fn reject(self, err: impl Into<ErrorObject>) {
        let _ = self.respond.send(MethodResponse::error(self.id, err));
    }

    /// Returns the method name used for notifications.
    pub const fn method_name(&self) -> &'static str {
        self.method
    }

    /// Returns the subscription id.
    pub const fn subscription_id(&self) -> &SubscriptionId {
        &self.sub_id
    }

    /// Returns the id of the connection.
    pub fn connection_id(&self) -> ConnectionId {
        self.conn.id
    }
}

/// An accepted subscription.
///
/// Dropping it closes the subscription.
#[derive(Debug)]
pub struct SubscriptionSink {
    method: &'static str,
    sub_id: SubscriptionId,
    /// Closed when the subscription is removed from the connection.
    close: watch::Sender<()>,
    conn: Arc<Connection>,
    _permit: OwnedSemaphorePermit,
}

impl SubscriptionSink {
    /// Sends a notification with the given result, waiting for capacity on the connection.
    ///
    /// Fails if the subscription or the connection was closed, or if `result` fails to serialize.
    pub async fn send<T: Serialize + ?Sized>(&self, result: &T) -> Result<(), DisconnectError> {
        if self.is_closed() {
            return Err(DisconnectError)
        }
        let json = notification(self.method, &self.sub_id, result).map_err(|err| {
            tracing::error!(target: "rpc::jsonrpc", %err, "failed to serialize notification");
            DisconnectError
        })?;
        self.conn.tx.send(json).await.map_err(|_| DisconnectError)
    }

    /// Sends a notification with the given result if the connection has capacity for it.
    ///
    /// Fails with [`TrySendError::Closed`] if `result` fails to serialize, like [`Self::send`].
    pub fn try_send<T: Serialize + ?Sized>(&self, result: &T) -> Result<(), TrySendError> {
        if self.is_closed() {
            return Err(TrySendError::Closed)
        }
        let permit = self.conn.tx.try_reserve().map_err(|err| match err {
            mpsc::error::TrySendError::Full(()) => TrySendError::Full,
            mpsc::error::TrySendError::Closed(()) => TrySendError::Closed,
        })?;
        let json = notification(self.method, &self.sub_id, result).map_err(|err| {
            tracing::error!(target: "rpc::jsonrpc", %err, "failed to serialize notification");
            TrySendError::Closed
        })?;
        permit.send(json);
        Ok(())
    }

    /// Returns the number of messages the connection can queue before sends wait.
    pub fn capacity(&self) -> usize {
        self.conn.tx.capacity()
    }

    /// Returns the maximum number of messages the connection can queue.
    pub fn max_capacity(&self) -> usize {
        self.conn.tx.max_capacity()
    }

    /// Resolves once the subscription or the connection is closed.
    pub async fn closed(&self) {
        tokio::select! {
            _ = self.close.closed() => {}
            _ = self.conn.tx.closed() => {}
        }
    }

    /// Returns `true` if the subscription or the connection is closed.
    pub fn is_closed(&self) -> bool {
        self.close.is_closed() || self.conn.tx.is_closed()
    }

    /// Returns the method name used for notifications.
    pub const fn method_name(&self) -> &'static str {
        self.method
    }

    /// Returns the subscription id.
    pub const fn subscription_id(&self) -> &SubscriptionId {
        &self.sub_id
    }

    /// Returns the id of the connection.
    pub fn connection_id(&self) -> ConnectionId {
        self.conn.id
    }
}

impl Drop for SubscriptionSink {
    fn drop(&mut self) {
        let mut subscriptions = self.conn.subscriptions.lock().unwrap();
        // Only remove our own entry, the id may have been reused after an unsubscribe.
        if subscriptions
            .get(&self.sub_id)
            .is_some_and(|(_, rx)| rx.same_channel(&self.close.subscribe()))
        {
            subscriptions.remove(&self.sub_id);
        }
    }
}

fn notification<T: Serialize + ?Sized>(
    method: &str,
    sub_id: &SubscriptionId,
    result: &T,
) -> Result<String, serde_json::Error> {
    #[derive(Serialize)]
    struct Notification<'a, T: ?Sized> {
        jsonrpc: &'static str,
        method: &'a str,
        params: Params<'a, T>,
    }

    #[derive(Serialize)]
    struct Params<'a, T: ?Sized> {
        subscription: &'a SubscriptionId,
        result: &'a T,
    }

    serde_json::to_string(&Notification {
        jsonrpc: "2.0",
        method,
        params: Params { subscription: sub_id, result },
    })
}

/// Error returned when a subscription cannot be accepted or notified.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("subscription closed")]
pub struct DisconnectError;

/// Error returned by [`SubscriptionSink::try_send`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TrySendError {
    /// The subscription or the connection was closed.
    #[error("subscription closed")]
    Closed,
    /// The connection has no capacity for another message.
    #[error("connection buffer full")]
    Full,
}
