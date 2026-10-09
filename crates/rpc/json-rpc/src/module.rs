use crate::{
    connection::handle_message, subscription::Connection, BatchRequestConfig, ConnectionId,
    ErrorCode, ErrorObject, Id, IntoSubscriptionResult, MethodResponse, Params,
    PendingSubscriptionSink, RandomIntegerIdProvider, RpcService, SubscriptionId,
};
use bytes::Bytes;
use futures_util::future::BoxFuture;
use http::Extensions;
use rustc_hash::FxHashMap;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::value::RawValue;
use std::{collections::hash_map::Entry, fmt, future::Future, sync::Arc};
use tokio::sync::mpsc;

/// A set of registered methods.
///
/// Cloning is cheap; the methods are only copied when a clone is modified.
#[derive(Clone, Default)]
pub struct RpcModule(Arc<FxHashMap<&'static str, Callback>>);

impl RpcModule {
    /// Creates an empty set of methods.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a synchronous method.
    ///
    /// The method runs on the connection task, so it must not block.
    pub fn register_method<R, F>(
        &mut self,
        name: &'static str,
        callback: F,
    ) -> Result<(), RegisterMethodError>
    where
        R: IntoResponse,
        F: Fn(Params, &Extensions) -> R + Send + Sync + 'static,
    {
        self.insert(
            name,
            Callback::Sync(Arc::new(move |id, params, extensions, max_size| {
                callback(params, extensions).into_response(id, max_size)
            })),
        )
    }

    /// Registers an asynchronous method.
    pub fn register_async_method<R, F, Fut>(
        &mut self,
        name: &'static str,
        callback: F,
    ) -> Result<(), RegisterMethodError>
    where
        R: IntoResponse,
        F: Fn(Params, Extensions) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        self.insert(
            name,
            Callback::Async(Arc::new(move |id, params, extensions, max_size| {
                let fut = callback(params, extensions);
                Box::pin(async move { fut.await.into_response(id, max_size) })
            })),
        )
    }

    /// Registers a method that runs on the blocking thread pool.
    pub fn register_blocking_method<R, F>(
        &mut self,
        name: &'static str,
        callback: F,
    ) -> Result<(), RegisterMethodError>
    where
        R: IntoResponse,
        F: Fn(Params, Extensions) -> R + Send + Sync + 'static,
    {
        let callback = Arc::new(callback);
        self.insert(
            name,
            Callback::Async(Arc::new(move |id, params, extensions, max_size| {
                let callback = callback.clone();
                let err_id = id.clone();
                Box::pin(async move {
                    tokio::task::spawn_blocking(move || {
                        callback(params, extensions).into_response(id, max_size)
                    })
                    .await
                    .unwrap_or_else(|_| MethodResponse::error(err_id, ErrorCode::InternalError))
                })
            })),
        )
    }

    /// Registers a subscription whose callback runs in a new task.
    ///
    /// `notification` is the method name of notifications. Errors returned by the callback are
    /// logged.
    pub fn register_subscription<R, F, Fut>(
        &mut self,
        subscribe: &'static str,
        notification: &'static str,
        unsubscribe: &'static str,
        callback: F,
    ) -> Result<(), RegisterMethodError>
    where
        R: IntoSubscriptionResult,
        F: Fn(Params, PendingSubscriptionSink, Extensions) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        self.register_subscription_raw(subscribe, notification, unsubscribe, move |params, pending, extensions| {
            let fut = callback(params, pending, extensions);
            tokio::spawn(async move {
                if let Err(err) = fut.await.into_subscription_result() {
                    tracing::debug!(target: "rpc::jsonrpc", method = subscribe, err = err.as_str(), "subscription failed");
                }
            });
        })
    }

    /// Registers a subscription whose callback runs on the connection task.
    ///
    /// The callback must not block; it is expected to spawn a task that drives the subscription.
    pub fn register_subscription_raw<R, F>(
        &mut self,
        subscribe: &'static str,
        notification: &'static str,
        unsubscribe: &'static str,
        callback: F,
    ) -> Result<(), RegisterMethodError>
    where
        R: IntoSubscriptionResult,
        F: Fn(Params, PendingSubscriptionSink, Extensions) -> R + Send + Sync + 'static,
    {
        if subscribe == unsubscribe {
            return Err(RegisterMethodError::SubscriptionNameConflict(subscribe.into()))
        }
        if self.0.contains_key(subscribe) {
            return Err(RegisterMethodError::AlreadyRegistered(subscribe.into()))
        }
        self.insert(unsubscribe, Callback::Unsubscription { method: unsubscribe })?;
        self.insert(
            subscribe,
            Callback::Subscription {
                notification,
                unsubscribe,
                callback: Arc::new(move |params, pending, extensions| {
                    if let Err(err) = callback(params, pending, extensions).into_subscription_result() {
                        tracing::debug!(target: "rpc::jsonrpc", method = subscribe, err = err.as_str(), "subscription failed");
                    }
                }),
            },
        )
    }

    /// Registers `alias` as another name for the existing method `name`.
    pub fn register_alias(
        &mut self,
        alias: &'static str,
        name: &'static str,
    ) -> Result<(), RegisterMethodError> {
        let callback = self
            .method(name)
            .cloned()
            .ok_or_else(|| RegisterMethodError::MethodNotFound(name.into()))?;
        self.insert(alias, callback)
    }

    /// Merges `other` into `self`, failing without changes if any method is already registered.
    pub fn merge(&mut self, other: Self) -> Result<(), RegisterMethodError> {
        if let Some(name) = other.method_names().find(|name| self.0.contains_key(name)) {
            return Err(RegisterMethodError::AlreadyRegistered(name.into()))
        }
        let methods = Arc::make_mut(&mut self.0);
        match Arc::try_unwrap(other.0) {
            Ok(other) => methods.extend(other),
            Err(other) => methods.extend(other.iter().map(|(k, v)| (*k, v.clone()))),
        }
        Ok(())
    }

    /// Removes the method with the given name, returning `true` if it existed.
    pub fn remove_method(&mut self, name: &str) -> bool {
        self.0.contains_key(name) && Arc::make_mut(&mut self.0).remove(name).is_some()
    }

    /// Keeps only the methods whose name matches `f`.
    pub fn retain(&mut self, mut f: impl FnMut(&str) -> bool) {
        if self.0.keys().any(|name| !f(name)) {
            Arc::make_mut(&mut self.0).retain(|name, _| f(name));
        }
    }

    /// Returns `true` if a method with the given name exists.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains_key(name)
    }

    /// Returns the names of all methods.
    pub fn method_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.keys().copied()
    }

    /// Handles a raw JSON-RPC request or batch without a connection.
    ///
    /// Returns `None` if the request only contained notifications.
    pub async fn raw_json_request(&self, request: &str) -> Option<String> {
        let service = RpcService::new(self.clone(), usize::MAX, None);
        let msg = Bytes::copy_from_slice(request.as_bytes());
        handle_message(&service, msg, usize::MAX, BatchRequestConfig::Unlimited, &Extensions::new())
            .await
            .map(|(json, _)| json)
    }

    /// Calls a method without a connection and deserializes its result.
    ///
    /// `params` must serialize to an array or object, or to `null` for no parameters.
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T, MethodsError> {
        let service = RpcService::new(self.clone(), usize::MAX, None);
        let json = call_json(&service, method, params).await?.0;
        parse_result(&json)
    }

    /// Subscribes without a connection, buffering up to `buffer` notifications.
    ///
    /// The subscription closes when the returned [`ModuleSubscription`] is dropped.
    pub async fn subscribe(
        &self,
        method: &str,
        params: impl Serialize,
        buffer: usize,
    ) -> Result<ModuleSubscription, MethodsError> {
        let (tx, rx) = mpsc::channel(buffer.max(1));
        let id_provider = Arc::new(RandomIntegerIdProvider);
        let conn = Connection::new(ConnectionId::next(), tx, u32::MAX, id_provider);
        let service = RpcService::new(self.clone(), usize::MAX, Some(Arc::new(conn)));
        let (json, on_sent) = call_json(&service, method, params).await?;
        let sub_id = parse_result(&json)?;
        for tx in on_sent {
            let _ = tx.send(());
        }
        Ok(ModuleSubscription { sub_id, rx })
    }

    pub(crate) fn method(&self, name: &str) -> Option<&Callback> {
        self.0.get(name)
    }

    fn insert(
        &mut self,
        name: &'static str,
        callback: Callback,
    ) -> Result<(), RegisterMethodError> {
        match Arc::make_mut(&mut self.0).entry(name) {
            Entry::Occupied(_) => Err(RegisterMethodError::AlreadyRegistered(name.into())),
            Entry::Vacant(entry) => {
                entry.insert(callback);
                Ok(())
            }
        }
    }
}

impl fmt::Debug for RpcModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.method_names()).finish()
    }
}

type SyncCallback = dyn Fn(Id, Params, &Extensions, usize) -> MethodResponse + Send + Sync;
type AsyncCallback =
    dyn Fn(Id, Params, Extensions, usize) -> BoxFuture<'static, MethodResponse> + Send + Sync;
type SubscriptionCallback = dyn Fn(Params, PendingSubscriptionSink, Extensions) + Send + Sync;

#[derive(Clone)]
pub(crate) enum Callback {
    Sync(Arc<SyncCallback>),
    Async(Arc<AsyncCallback>),
    Subscription {
        notification: &'static str,
        unsubscribe: &'static str,
        callback: Arc<SubscriptionCallback>,
    },
    Unsubscription {
        method: &'static str,
    },
}

/// Error returned by [`RpcModule::call`] and [`RpcModule::subscribe`].
#[derive(Debug, thiserror::Error)]
pub enum MethodsError {
    /// The method returned an error.
    #[error(transparent)]
    JsonRpc(#[from] ErrorObject),
    /// The parameters or the result failed to (de)serialize.
    #[error(transparent)]
    Parse(#[from] serde_json::Error),
}

/// A subscription created by [`RpcModule::subscribe`].
#[derive(Debug)]
pub struct ModuleSubscription {
    sub_id: SubscriptionId,
    rx: mpsc::Receiver<String>,
}

impl ModuleSubscription {
    /// Returns the subscription id.
    pub const fn subscription_id(&self) -> &SubscriptionId {
        &self.sub_id
    }

    /// Receives the next notification result, or `None` once the subscription closed.
    pub async fn next<T: DeserializeOwned>(&mut self) -> Option<Result<T, serde_json::Error>> {
        #[derive(Deserialize)]
        struct Notification<'a> {
            #[serde(borrow)]
            params: NotificationParams<'a>,
        }

        #[derive(Deserialize)]
        struct NotificationParams<'a> {
            #[serde(borrow)]
            result: &'a RawValue,
        }

        let json = self.rx.recv().await?;
        Some(
            serde_json::from_str::<Notification<'_>>(&json)
                .and_then(|n| serde_json::from_str(n.params.result.get())),
        )
    }
}

/// Error returned when registering a method fails.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RegisterMethodError {
    /// A method with the same name is already registered.
    #[error("method `{0}` is already registered")]
    AlreadyRegistered(String),
    /// The subscribe and unsubscribe methods have the same name.
    #[error("subscription method `{0}` conflicts with its unsubscribe method")]
    SubscriptionNameConflict(String),
    /// The aliased method does not exist.
    #[error("method `{0}` not found")]
    MethodNotFound(String),
}

/// Sends a request for `method` to `service` and returns the response.
async fn call_json(
    service: &RpcService,
    method: &str,
    params: impl Serialize,
) -> Result<(String, Vec<tokio::sync::oneshot::Sender<()>>), MethodsError> {
    #[derive(Serialize)]
    struct Request<'a, P> {
        jsonrpc: &'static str,
        id: u8,
        method: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        params: Option<P>,
    }

    let params = serde_json::to_value(params)?;
    let params = (!params.is_null()).then_some(params);
    let request = serde_json::to_vec(&Request { jsonrpc: "2.0", id: 0, method, params })?;
    let config = BatchRequestConfig::Unlimited;
    let response =
        handle_message(service, request.into(), usize::MAX, config, &Extensions::new()).await;
    Ok(response.expect("requests with an id get a response"))
}

/// Parses the result of a JSON-RPC response.
fn parse_result<T: DeserializeOwned>(json: &str) -> Result<T, MethodsError> {
    #[derive(Deserialize)]
    struct Response<'a> {
        #[serde(borrow)]
        result: Option<&'a RawValue>,
        error: Option<ErrorObject>,
    }

    let response = serde_json::from_str::<Response<'_>>(json)?;
    if let Some(err) = response.error {
        return Err(err.into())
    }
    Ok(serde_json::from_str(response.result.map_or("null", RawValue::get))?)
}

/// Converts the output of a method into a response.
pub trait IntoResponse {
    /// Serializes `self` as the response to the request with the given id.
    fn into_response(self, id: Id, max_size: usize) -> MethodResponse;
}

impl<T: Serialize, E: Into<ErrorObject>> IntoResponse for Result<T, E> {
    fn into_response(self, id: Id, max_size: usize) -> MethodResponse {
        MethodResponse::from_result(id, self, max_size)
    }
}

impl IntoResponse for MethodResponse {
    fn into_response(self, _id: Id, _max_size: usize) -> MethodResponse {
        self
    }
}

macro_rules! impl_into_response {
    ($($t:ty),*) => {$(
        impl IntoResponse for $t {
            fn into_response(self, id: Id, max_size: usize) -> MethodResponse {
                MethodResponse::response(id, &self, max_size)
            }
        }
    )*};
}

impl_into_response!(
    (),
    &str,
    String,
    bool,
    char,
    u8,
    u16,
    u32,
    u64,
    u128,
    usize,
    i8,
    i16,
    i32,
    i64,
    i128,
    isize,
    f32,
    f64,
    serde_json::Value
);

impl<T: Serialize> IntoResponse for Option<T> {
    fn into_response(self, id: Id, max_size: usize) -> MethodResponse {
        MethodResponse::response(id, &self, max_size)
    }
}

impl<T: Serialize> IntoResponse for Vec<T> {
    fn into_response(self, id: Id, max_size: usize) -> MethodResponse {
        MethodResponse::response(id, &self, max_size)
    }
}

impl<T: Serialize, const N: usize> IntoResponse for [T; N] {
    fn into_response(self, id: Id, max_size: usize) -> MethodResponse {
        MethodResponse::response(id, &self[..], max_size)
    }
}

impl IntoResponse for ErrorObject {
    fn into_response(self, id: Id, _max_size: usize) -> MethodResponse {
        MethodResponse::error(id, self)
    }
}

/// The output of a method generated by the `rpc` macro, or the error from parsing its parameters.
#[doc(hidden)]
#[derive(Debug)]
pub enum MethodResult<R> {
    /// The method output.
    Ok(R),
    /// The parameters could not be parsed.
    Err(ErrorObject),
}

impl<R: IntoResponse> IntoResponse for MethodResult<R> {
    fn into_response(self, id: Id, max_size: usize) -> MethodResponse {
        match self {
            Self::Ok(output) => output.into_response(id, max_size),
            Self::Err(err) => MethodResponse::error(id, err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_merge() {
        let mut a = RpcModule::new();
        a.register_method("a", |_, _| "a").unwrap();
        assert_eq!(
            a.register_method("a", |_, _| "a").unwrap_err(),
            RegisterMethodError::AlreadyRegistered("a".into())
        );
        a.register_alias("b", "a").unwrap();

        let mut c = RpcModule::new();
        c.register_method("c", |_, _| "c").unwrap();
        c.register_method("b", |_, _| "b").unwrap();

        let mut methods = a;
        assert!(methods.merge(c.clone()).is_err());
        assert!(!methods.contains("c"));
        assert!(c.remove_method("b"));
        methods.merge(c).unwrap();

        let mut names = methods.method_names().collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(names, ["a", "b", "c"]);

        methods.retain(|name| name != "b");
        assert!(!methods.contains("b"));
        assert!(methods.contains("a"));
    }

    #[test]
    fn subscription_names() {
        let mut m = RpcModule::new();
        m.register_subscription("sub", "notif", "unsub", |_, _, _| async { Ok(()) }).unwrap();
        assert!(m
            .register_subscription("x", "notif", "unsub", |_, _, _| async { Ok(()) })
            .is_err());
        assert!(m.register_subscription("y", "y", "y", |_, _, _| async { Ok(()) }).is_err());
        assert!(m.contains("unsub"));
    }

    #[tokio::test]
    async fn raw_json_request() {
        let mut m = RpcModule::new();
        m.register_method("sync", |params, _| params.one::<u64>().map(|n| n + 5)).unwrap();
        m.register_async_method("async", |_, _| async { Ok::<_, ErrorObject>(5) }).unwrap();
        assert_eq!(
            m.raw_json_request(r#"{"jsonrpc":"2.0","id":1,"method":"sync","params":[1]}"#)
                .await
                .unwrap(),
            r#"{"jsonrpc":"2.0","id":1,"result":6}"#
        );
        assert_eq!(
            m.raw_json_request(
                r#"[{"jsonrpc":"2.0","id":1,"method":"async"},{"jsonrpc":"2.0","method":"x"},{"jsonrpc":"2.0","id":2,"method":"x"}]"#
            )
            .await
            .unwrap(),
            r#"[{"jsonrpc":"2.0","id":1,"result":5},{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"Method not found"}}]"#
        );
        assert_eq!(m.raw_json_request(r#"{"jsonrpc":"2.0","method":"x"}"#).await, None);
    }

    #[tokio::test]
    async fn call_and_subscribe() {
        let mut m = RpcModule::new();
        m.register_method("add", |params, _| params.parse::<(u64, u64)>().map(|(a, b)| a + b))
            .unwrap();
        m.register_method("none", |params, _| params.is_none()).unwrap();
        m.register_subscription("sub", "notif", "unsub", |params, pending, _| async move {
            let n = match params.one::<u64>() {
                Ok(n) => n,
                Err(err) => {
                    pending.reject(err);
                    return Ok(())
                }
            };
            let sink = pending.accept().await?;
            for i in 0..n {
                sink.send(&i).await?;
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(m.call::<u64>("add", (1, 2)).await.unwrap(), 3);
        assert!(m.call::<bool>("none", ()).await.unwrap());
        let err = m.call::<u64>("add", ["x"]).await.unwrap_err();
        assert!(matches!(err, MethodsError::JsonRpc(err) if err.code() == -32602));
        let err = m.call::<u64>("missing", ()).await.unwrap_err();
        assert!(matches!(err, MethodsError::JsonRpc(err) if err.code() == -32601));
        assert!(matches!(m.call::<String>("add", (1, 2)).await, Err(MethodsError::Parse(_))));

        let mut sub = m.subscribe("sub", [2], 1).await.unwrap();
        assert_eq!(sub.next::<u64>().await.unwrap().unwrap(), 0);
        assert_eq!(sub.next::<u64>().await.unwrap().unwrap(), 1);
        assert!(sub.next::<u64>().await.is_none());
        let err = m.subscribe("sub", ["x"], 1).await.unwrap_err();
        assert!(matches!(err, MethodsError::JsonRpc(err) if err.code() == -32602));
    }
}
