use crate::{
    connection::handle_message, subscription::IntoSubscriptionResult, ErrorObject, Id,
    MethodResponse, Params, PendingSubscriptionSink, RpcService,
};
use bytes::Bytes;
use futures_util::future::BoxFuture;
use rustc_hash::FxHashMap;
use serde::Serialize;
use std::{
    collections::hash_map::Entry,
    fmt,
    future::Future,
    ops::{Deref, DerefMut},
    sync::Arc,
};

/// A set of registered methods.
///
/// Cloning is cheap; the methods are only copied when a clone is modified.
#[derive(Clone, Default)]
pub struct Methods(Arc<FxHashMap<&'static str, MethodCallback>>);

impl Methods {
    /// Creates an empty set of methods.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a method, failing if one with the same name exists.
    pub fn verify_and_insert(
        &mut self,
        name: &'static str,
        callback: MethodCallback,
    ) -> Result<&mut MethodCallback, RegisterMethodError> {
        match Arc::make_mut(&mut self.0).entry(name) {
            Entry::Occupied(_) => Err(RegisterMethodError::AlreadyRegistered(name.into())),
            Entry::Vacant(entry) => Ok(entry.insert(callback)),
        }
    }

    /// Merges `other` into `self`, failing without changes if any method is already registered.
    pub fn merge(&mut self, other: impl Into<Self>) -> Result<(), RegisterMethodError> {
        let other = other.into();
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

    /// Returns the method with the given name.
    pub fn method(&self, name: &str) -> Option<&MethodCallback> {
        self.0.get(name)
    }

    /// Removes the method with the given name.
    pub fn remove_method(&mut self, name: &str) -> Option<MethodCallback> {
        if !self.0.contains_key(name) {
            return None
        }
        Arc::make_mut(&mut self.0).remove(name)
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
        handle_message(&service, Bytes::copy_from_slice(request.as_bytes()), usize::MAX).await
    }
}

impl fmt::Debug for Methods {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.method_names()).finish()
    }
}

/// A set of methods that share a context.
#[derive(Clone)]
pub struct RpcModule<Ctx> {
    ctx: Arc<Ctx>,
    methods: Methods,
}

impl<Ctx> RpcModule<Ctx> {
    /// Creates an empty module with the given context.
    pub fn new(ctx: Ctx) -> Self {
        Self::from_arc(Arc::new(ctx))
    }

    /// Creates an empty module with the given shared context.
    pub fn from_arc(ctx: Arc<Ctx>) -> Self {
        Self { ctx, methods: Methods::new() }
    }

    /// Drops the context, keeping the methods.
    pub fn remove_context(self) -> RpcModule<()> {
        RpcModule { ctx: Arc::new(()), methods: self.methods }
    }
}

impl<Ctx: Send + Sync + 'static> RpcModule<Ctx> {
    /// Registers a synchronous method.
    pub fn register_method<R, F>(
        &mut self,
        name: &'static str,
        callback: F,
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        R: IntoResponse,
        F: Fn(Params, &Ctx) -> R + Send + Sync + 'static,
    {
        let ctx = self.ctx.clone();
        self.methods.verify_and_insert(
            name,
            MethodCallback(Callback::Sync(Arc::new(move |id, params, max_size| {
                callback(params, &ctx).into_response(id, max_size)
            }))),
        )
    }

    /// Registers an asynchronous method.
    pub fn register_async_method<R, F, Fut>(
        &mut self,
        name: &'static str,
        callback: F,
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        R: IntoResponse,
        F: Fn(Params, Arc<Ctx>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        let ctx = self.ctx.clone();
        self.methods.verify_and_insert(
            name,
            MethodCallback(Callback::Async(Arc::new(move |id, params, max_size| {
                let fut = callback(params, ctx.clone());
                Box::pin(async move { fut.await.into_response(id, max_size) })
            }))),
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
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        R: IntoSubscriptionResult,
        F: Fn(Params, PendingSubscriptionSink, Arc<Ctx>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = R> + Send + 'static,
    {
        self.register_subscription_raw(subscribe, notification, unsubscribe, move |params, pending, ctx| {
            let fut = callback(params, pending, ctx);
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
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        R: IntoSubscriptionResult,
        F: Fn(Params, PendingSubscriptionSink, Arc<Ctx>) -> R + Send + Sync + 'static,
    {
        if subscribe == unsubscribe {
            return Err(RegisterMethodError::SubscriptionNameConflict(subscribe.into()))
        }
        if self.methods.method(subscribe).is_some() {
            return Err(RegisterMethodError::AlreadyRegistered(subscribe.into()))
        }
        self.methods.verify_and_insert(
            unsubscribe,
            MethodCallback(Callback::Unsubscription { method: unsubscribe }),
        )?;
        let ctx = self.ctx.clone();
        self.methods.verify_and_insert(
            subscribe,
            MethodCallback(Callback::Subscription {
                notification,
                unsubscribe,
                callback: Arc::new(move |params, pending| {
                    if let Err(err) = callback(params, pending, ctx.clone()).into_subscription_result() {
                        tracing::debug!(target: "rpc::jsonrpc", method = subscribe, err = err.as_str(), "subscription failed");
                    }
                }),
            }),
        )
    }

    /// Registers `alias` as another name for the existing method `name`.
    pub fn register_alias(
        &mut self,
        alias: &'static str,
        name: &'static str,
    ) -> Result<(), RegisterMethodError> {
        let callback = self
            .methods
            .method(name)
            .cloned()
            .ok_or_else(|| RegisterMethodError::MethodNotFound(name.into()))?;
        self.methods.verify_and_insert(alias, callback)?;
        Ok(())
    }
}

impl<Ctx> Deref for RpcModule<Ctx> {
    type Target = Methods;

    fn deref(&self) -> &Methods {
        &self.methods
    }
}

impl<Ctx> DerefMut for RpcModule<Ctx> {
    fn deref_mut(&mut self) -> &mut Methods {
        &mut self.methods
    }
}

impl<Ctx> From<RpcModule<Ctx>> for Methods {
    fn from(module: RpcModule<Ctx>) -> Self {
        module.methods
    }
}

impl<Ctx> fmt::Debug for RpcModule<Ctx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.methods.fmt(f)
    }
}

/// A registered method.
#[derive(Clone)]
pub struct MethodCallback(pub(crate) Callback);

impl fmt::Debug for MethodCallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.0 {
            Callback::Sync(_) => "Sync",
            Callback::Async(_) => "Async",
            Callback::Subscription { .. } => "Subscription",
            Callback::Unsubscription { .. } => "Unsubscription",
        })
    }
}

type SyncCallback = dyn Fn(Id, Params, usize) -> MethodResponse + Send + Sync;
type AsyncCallback = dyn Fn(Id, Params, usize) -> BoxFuture<'static, MethodResponse> + Send + Sync;
type SubscriptionCallback = dyn Fn(Params, PendingSubscriptionSink) + Send + Sync;

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

impl_into_response!(&str, String, bool);

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
        let mut a = RpcModule::new(());
        a.register_method("a", |_, _| "a").unwrap();
        assert_eq!(
            a.register_method("a", |_, _| "a").unwrap_err(),
            RegisterMethodError::AlreadyRegistered("a".into())
        );
        a.register_alias("b", "a").unwrap();

        let mut c = RpcModule::new(());
        c.register_method("c", |_, _| "c").unwrap();
        c.register_method("b", |_, _| "b").unwrap();

        let mut methods = Methods::from(a);
        assert!(methods.merge(c.clone()).is_err());
        assert!(methods.method("c").is_none());
        assert!(c.remove_method("b").is_some());
        methods.merge(c).unwrap();

        let mut names = methods.method_names().collect::<Vec<_>>();
        names.sort_unstable();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn subscription_names() {
        let mut m = RpcModule::new(());
        m.register_subscription_raw("sub", "notif", "unsub", |_, _, _| ()).unwrap();
        assert!(m.register_subscription_raw("x", "notif", "unsub", |_, _, _| ()).is_err());
        assert!(m.register_subscription_raw("y", "y", "y", |_, _, _| ()).is_err());
        assert!(m.method("unsub").is_some());
    }

    #[tokio::test]
    async fn raw_json_request() {
        let mut m = RpcModule::new(5u64);
        m.register_method("sync", |params, ctx| params.one::<u64>().map(|n| n + ctx)).unwrap();
        m.register_async_method("async", |_, ctx| async move { Ok::<_, ErrorObject>(*ctx) })
            .unwrap();
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
