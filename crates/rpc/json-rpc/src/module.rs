use crate::{
    connection::handle_message, BatchRequestConfig, ErrorCode, ErrorObject, Id,
    IntoSubscriptionResult, MethodResponse, Params, PendingSubscriptionSink, RpcService,
};
use bytes::Bytes;
use futures_util::future::BoxFuture;
use http::Extensions;
use rustc_hash::FxHashMap;
use serde::Serialize;
use std::{collections::hash_map::Entry, fmt, future::Future, sync::Arc};

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
}
