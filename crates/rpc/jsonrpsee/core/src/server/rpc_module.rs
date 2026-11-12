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

use std::{
    collections::hash_map::Entry,
    fmt::Debug,
    future::Future,
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use crate::{
    error::RegisterMethodError,
    id_providers::RandomIntegerIdProvider,
    server::{
        helpers::MethodSink,
        subscription::{
            notification_prefix, sub_message_to_json, BoundedSubscriptions,
            IntoSubscriptionCloseResponse, PendingSubscriptionSink, Subscribers,
            SubscriptionCloseResponse, SubscriptionKey, SubscriptionPermit, SubscriptionState,
        },
        MethodResponse, ResponsePayload, ResponseSizeHint, LOG_TARGET,
    },
};
use futures_util::{future::BoxFuture, FutureExt};
use jsonrpsee_types::{
    error::{ErrorCode, ErrorObject},
    Id, Params, Request, SubscriptionId as RpcSubscriptionId,
};
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot};

use super::{sub_err_to_json, IntoResponse};

/// A `MethodCallback` is an RPC endpoint, callable with a standard JSON-RPC request,
/// implemented as a function pointer to a `Fn` function taking four arguments:
/// the `id`, `params`, a channel the function uses to communicate the result (or error)
/// back to `jsonrpsee`, and the connection ID (useful for the websocket transport).
pub type SyncMethod = Arc<dyn Send + Sync + Fn(Id, Params, MaxResponseSize) -> MethodResponse>;
/// Similar to [`SyncMethod`], but represents an asynchronous handler.
pub type AsyncMethod<'a> = Arc<
    dyn Send
        + Sync
        + Fn(Id<'a>, Params<'a>, ConnectionId, MaxResponseSize) -> BoxFuture<'a, MethodResponse>,
>;

/// Method callback for subscriptions.
pub type SubscriptionMethod<'a> = Arc<
    dyn Send
        + Sync
        + Fn(Id, Params, MethodSink, SubscriptionState) -> BoxFuture<'a, MethodResponse>,
>;
// Method callback to unsubscribe.
type UnsubscriptionMethod =
    Arc<dyn Send + Sync + Fn(Id, Params, ConnectionId, MaxResponseSize) -> MethodResponse>;

/// Connection ID.
#[derive(
    Debug, Copy, Clone, PartialEq, Eq, Hash, Default, serde::Deserialize, serde::Serialize,
)]
pub struct ConnectionId(pub usize);

impl ConnectionId {
    /// Owner of subscriptions created by in-process calls such as
    /// [`Methods::raw_json_request`].
    ///
    /// Servers count connections upwards from zero and never reach this value, so no transport
    /// connection can unsubscribe an in-process subscription.
    pub const IN_PROCESS: Self = Self(usize::MAX);

    /// Returns an id that no other live connection in this process shares.
    ///
    /// Together with the subscription id this scopes a subscription, and servers of different
    /// transports can serve one shared subscription table, so the id has to be unique across every
    /// server in the process and not only within a single accept loop. Counting per server would
    /// give the first connection of each transport the same id, and an unsubscribe on one of them
    /// would then cancel a subscription of another.
    ///
    /// A `usize` cannot wrap in practice on 64-bit targets, so ids are never reused and never reach
    /// [`Self::IN_PROCESS`].
    pub fn next_unique() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl From<usize> for ConnectionId {
    fn from(id: usize) -> Self {
        Self(id)
    }
}

/// Max response size.
pub type MaxResponseSize = usize;

/// Callback wrapper that can be either sync or async.
#[derive(Clone)]
pub enum MethodCallback {
    /// Synchronous method handler.
    Sync(SyncMethod),
    /// Asynchronous method handler.
    Async(AsyncMethod<'static>),
    /// Subscription method handler.
    Subscription(SubscriptionMethod<'static>),
    /// Unsubscription method handler.
    Unsubscription(UnsubscriptionMethod),
}

impl Debug for MethodCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Async(_) => write!(f, "Async"),
            Self::Sync(_) => write!(f, "Sync"),
            Self::Subscription(_) => write!(f, "Subscription"),
            Self::Unsubscription(_) => write!(f, "Unsubscription"),
        }
    }
}

/// Reference-counted, clone-on-write collection of synchronous and asynchronous methods.
#[derive(Default, Debug, Clone)]
pub struct Methods {
    callbacks: Arc<FxHashMap<&'static str, MethodCallback>>,
}

impl Methods {
    /// Creates a new empty [`Methods`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Verifies that the method name is not already taken, and returns an error if it is.
    pub fn verify_method_name(&mut self, name: &'static str) -> Result<(), RegisterMethodError> {
        if self.callbacks.contains_key(name) {
            return Err(RegisterMethodError::AlreadyRegistered(name.into()));
        }

        Ok(())
    }

    /// Inserts the method callback for a given name, or returns an error if the name was already
    /// taken. On success it returns a mut reference to the [`MethodCallback`] just inserted.
    pub fn verify_and_insert(
        &mut self,
        name: &'static str,
        callback: MethodCallback,
    ) -> Result<&mut MethodCallback, RegisterMethodError> {
        match self.mut_callbacks().entry(name) {
            Entry::Occupied(_) => Err(RegisterMethodError::AlreadyRegistered(name.into())),
            Entry::Vacant(vacant) => Ok(vacant.insert(callback)),
        }
    }

    /// Helper for obtaining a mut ref to the callbacks HashMap.
    fn mut_callbacks(&mut self) -> &mut FxHashMap<&'static str, MethodCallback> {
        Arc::make_mut(&mut self.callbacks)
    }

    /// Merge two [`Methods`]'s by adding all [`MethodCallback`]s from `other` into `self`.
    /// Fails if any of the methods in `other` is present already.
    pub fn merge(&mut self, other: impl Into<Methods>) -> Result<(), RegisterMethodError> {
        let mut other = other.into();

        for name in other.callbacks.keys() {
            self.verify_method_name(name)?;
        }

        let callbacks = self.mut_callbacks();

        for (name, callback) in other.mut_callbacks().drain() {
            callbacks.insert(name, callback);
        }

        Ok(())
    }

    /// Returns the method callback.
    pub fn method(&self, method_name: &str) -> Option<&MethodCallback> {
        self.callbacks.get(method_name)
    }

    /// Make a request (JSON-RPC method call or subscription) by using raw JSON.
    ///
    /// Returns the raw JSON response to the call and a stream to receive notifications if the call
    /// was a subscription.
    ///
    /// # Examples
    ///
    /// ```
    /// #[tokio::main]
    /// async fn main() {
    ///     use jsonrpsee::{RpcModule, SubscriptionMessage};
    ///     use jsonrpsee::types::{response::Success, Response};
    ///     use jsonrpsee::core::to_json_raw_value;
    ///     use futures_util::StreamExt;
    ///
    ///     let mut module = RpcModule::new(());
    ///     module.register_subscription("hi", "hi", "goodbye", |_, pending, _| async {
    ///         let sink = pending.accept().await?;
    ///
    ///         // see comment above.
    ///         let msg = to_json_raw_value(&"one answer").unwrap();
    ///         sink.send(msg).await?;
    ///
    ///         Ok(())
    ///     }).unwrap();
    ///     let (resp, mut stream) = module.raw_json_request(r#"{"jsonrpc":"2.0","method":"hi","id":0}"#, 1).await.unwrap();
    ///     // If the response is an error converting it to `Success` will fail.
    ///     let resp: Success<u64> = serde_json::from_str::<Response<u64>>(&resp).unwrap().try_into().unwrap();
    ///     let sub_resp = stream.recv().await.unwrap();
    ///     assert_eq!(
    ///         format!(r#"{{"jsonrpc":"2.0","method":"hi","params":{{"subscription":{},"result":"one answer"}}}}"#, resp.result),
    ///         sub_resp
    ///     );
    /// }
    /// ```
    pub async fn raw_json_request(
        &self,
        request: &str,
        buf_size: usize,
    ) -> Result<(String, mpsc::Receiver<String>), serde_json::Error> {
        tracing::trace!("[Methods::raw_json_request] Request: {:?}", request);
        let req: Request = serde_json::from_str(request)?;
        let (resp, rx) = self.inner_call(req, buf_size, mock_subscription_permit()).await;

        Ok((resp, rx))
    }

    /// Execute a callback.
    async fn inner_call(
        &self,
        req: Request<'_>,
        buf_size: usize,
        subscription_permit: SubscriptionPermit,
    ) -> (String, mpsc::Receiver<String>) {
        let (tx, mut rx) = mpsc::channel(buf_size);
        let Request { id, method, params, .. } = req;
        let params = Params::new(params.as_ref().map(|params| params.as_ref().get()));
        let max_response_size = usize::MAX;
        let conn_id = ConnectionId::IN_PROCESS;

        let response = match self.method(&method) {
            None => MethodResponse::error(id, ErrorObject::from(ErrorCode::MethodNotFound)),
            Some(MethodCallback::Sync(cb)) => (cb)(id, params, max_response_size),
            Some(MethodCallback::Async(cb)) => {
                (cb)(id.into_owned(), params.into_owned(), conn_id, max_response_size).await
            }
            Some(MethodCallback::Subscription(cb)) => {
                let conn_state = SubscriptionState {
                    conn_id,
                    id_provider: &RandomIntegerIdProvider,
                    subscription_permit,
                };
                let res = (cb)(id, params, MethodSink::new(tx.clone()), conn_state).await;

                // This message is not used because it's used for metrics so we discard in other to
                // not read once this is used for subscriptions.
                //
                // The same information is part of `res` above.
                let _ =
                    rx.recv().await.expect("Every call must at least produce one response; qed");

                res
            }
            Some(MethodCallback::Unsubscription(cb)) => {
                (cb)(id, params, conn_id, max_response_size)
            }
        };

        let rp = response.into_json();
        tracing::trace!(target: LOG_TARGET, "[Methods::inner_call] Method: {}, response: {}", method, rp);

        (rp, rx)
    }

    /// Returns an `Iterator` with all the method names registered on this server.
    pub fn method_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.callbacks.keys().copied()
    }
}

impl<Context> Deref for RpcModule<Context> {
    type Target = Methods;

    fn deref(&self) -> &Methods {
        &self.methods
    }
}

impl<Context> DerefMut for RpcModule<Context> {
    fn deref_mut(&mut self) -> &mut Methods {
        &mut self.methods
    }
}

/// Sets of JSON-RPC methods can be organized into "module"s that are in turn registered on the
/// server or, alternatively, merged with other modules to construct a cohesive API. [`RpcModule`]
/// wraps an additional context argument that can be used to access data during call execution.
#[derive(Debug, Clone)]
pub struct RpcModule<Context> {
    ctx: Arc<Context>,
    methods: Methods,
}

impl<Context> RpcModule<Context> {
    /// Create a new module with a given shared `Context`.
    pub fn new(ctx: Context) -> Self {
        Self::from_arc(Arc::new(ctx))
    }

    /// Create a new module from an already shared `Context`.
    ///
    /// This is useful if `Context` needs to be shared outside of an [`RpcModule`].
    pub fn from_arc(ctx: Arc<Context>) -> Self {
        Self { ctx, methods: Default::default() }
    }

    /// Transform a module into an `RpcModule<()>` (unit context).
    pub fn remove_context(self) -> RpcModule<()> {
        let mut module = RpcModule::new(());
        module.methods = self.methods;
        module
    }
}

impl<Context> From<RpcModule<Context>> for Methods {
    fn from(module: RpcModule<Context>) -> Methods {
        module.methods
    }
}

impl<Context: Send + Sync + 'static> RpcModule<Context> {
    /// Register a new synchronous RPC method, which computes the response with the given callback.
    ///
    /// ## Examples
    ///
    /// ```
    /// use jsonrpsee_core::server::RpcModule;
    ///
    /// let mut module = RpcModule::new(());
    /// module.register_method("say_hello", |_params, _ctx| "lo").unwrap();
    /// ```
    pub fn register_method<R, F>(
        &mut self,
        method_name: &'static str,
        callback: F,
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        Context: Send + Sync + 'static,
        R: IntoResponse + 'static,
        F: Fn(Params, &Context) -> R + Send + Sync + 'static,
    {
        let ctx = self.ctx.clone();
        let size_hint = ResponseSizeHint::new();
        self.methods.verify_and_insert(
            method_name,
            MethodCallback::Sync(Arc::new(move |id, params, max_response_size| {
                let rp = callback(params, &*ctx).into_response();
                MethodResponse::sized_response(id, rp, max_response_size, &size_hint)
            })),
        )
    }

    /// Removes the method if it exists.
    ///
    /// Be aware that a subscription consist of two methods, `subscribe` and `unsubscribe` and
    /// it's the caller responsibility to remove both `subscribe` and `unsubscribe` methods for
    /// subscriptions.
    pub fn remove_method(&mut self, method_name: &'static str) -> Option<MethodCallback> {
        self.methods.mut_callbacks().remove(method_name)
    }

    /// Register a new asynchronous RPC method, which computes the response with the given callback.
    ///
    /// ## Examples
    ///
    /// ```
    /// use jsonrpsee_core::server::RpcModule;
    ///
    /// let mut module = RpcModule::new(());
    /// module.register_async_method("say_hello", |_params, _ctx| async { "lo" }).unwrap();
    /// ```
    pub fn register_async_method<R, Fun, Fut>(
        &mut self,
        method_name: &'static str,
        callback: Fun,
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        R: IntoResponse + 'static,
        Fut: Future<Output = R> + Send,
        Fun: (Fn(Params<'static>, Arc<Context>) -> Fut) + Clone + Send + Sync + 'static,
    {
        let ctx = self.ctx.clone();
        let size_hint = Arc::new(ResponseSizeHint::new());
        self.methods.verify_and_insert(
            method_name,
            MethodCallback::Async(Arc::new(move |id, params, _, max_response_size| {
                let ctx = ctx.clone();
                let callback = callback.clone();
                let size_hint = size_hint.clone();

                let future = async move {
                    let rp = callback(params, ctx).await.into_response();
                    MethodResponse::sized_response(id, rp, max_response_size, &size_hint)
                };
                future.boxed()
            })),
        )
    }

    /// Register a new **blocking** synchronous RPC method, which computes the response with the
    /// given callback. Unlike the regular [`register_method`](RpcModule::register_method), this
    /// method runs on tokio's blocking thread pool, so it can block its thread and perform
    /// expensive computations.
    pub fn register_blocking_method<R, F>(
        &mut self,
        method_name: &'static str,
        callback: F,
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        Context: Send + Sync + 'static,
        R: IntoResponse + 'static,
        F: Fn(Params<'static>, Arc<Context>) -> R + Clone + Send + Sync + 'static,
    {
        let ctx = self.ctx.clone();
        let size_hint = Arc::new(ResponseSizeHint::new());
        self.methods.verify_and_insert(
            method_name,
            MethodCallback::Async(Arc::new(move |id, params, _, max_response_size| {
                let ctx = ctx.clone();
                let callback = callback.clone();
                let size_hint = size_hint.clone();
                let error_id = id.clone();

                tokio::task::spawn_blocking(move || {
                    let rp = callback(params, ctx).into_response();
                    MethodResponse::sized_response(id, rp, max_response_size, &size_hint)
                })
                .map(|result| {
                    result.unwrap_or_else(|err| {
                        tracing::error!(target: LOG_TARGET, "Join error for blocking RPC method: {:?}", err);
                        MethodResponse::error(error_id, ErrorObject::from(ErrorCode::InternalError))
                    })
                })
                .boxed()
            })),
        )
    }

    /// Register a new publish/subscribe interface using JSON-RPC notifications.
    ///
    /// It implements the [ethereum pubsub specification](https://geth.ethereum.org/docs/rpc/pubsub)
    /// with an option to choose custom subscription ID generation.
    ///
    /// Furthermore, it generates the `unsubscribe implementation` where a `bool` is used as
    /// the result to indicate whether the subscription was successfully unsubscribed to or not.
    /// For instance an `unsubscribe call` may fail if a non-existent subscription ID is used in the
    /// call.
    ///
    /// This method ensures that the `subscription_method_name` and `unsubscription_method_name` are
    /// unique. The `notif_method_name` argument sets the content of the `method` field in the
    /// JSON document that the server sends back to the client. The uniqueness of this value is
    /// not machine checked and it's up to the user to ensure it is not used in any other
    /// [`RpcModule`] used in the server.
    ///
    /// # Arguments
    ///
    /// * `subscription_method_name` - name of the method to call to initiate a subscription
    /// * `notif_method_name` - name of method to be used in the subscription payload (technically a
    ///   JSON-RPC notification)
    /// * `unsubscription_method` - name of the method to call to terminate a subscription
    /// * `callback` - A callback to invoke on each subscription; it takes three parameters:
    ///     - [`Params`]: JSON-RPC parameters in the subscription call.
    ///     - [`PendingSubscriptionSink`]: A pending subscription waiting to be accepted, in order
    ///       to send out messages on the subscription
    ///     - Context: Any type that can be embedded into the [`RpcModule`].
    ///
    /// # Returns
    ///
    /// An async block which returns something that implements
    /// [`crate::server::IntoSubscriptionCloseResponse`] which decides what action to take when
    /// the subscription ends whether such as to sent out another message on the subscription
    /// stream before closing down it.
    ///
    /// NOTE: The return value is ignored if [`PendingSubscriptionSink`] hasn't been called or is
    /// unsuccessful, as the subscription is not allowed to send out subscription notifications
    /// before the actual subscription has been established.
    ///
    /// This is implemented for `Result<T, E>` and `()`.
    ///
    /// It's recommended to use `Result` if you want to propagate the error as special error
    /// notification Another option is to implement
    /// [`crate::server::IntoSubscriptionCloseResponse`] if you want customized behaviour.
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
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use jsonrpsee_core::server::{RpcModule, SubscriptionMessage, SubscriptionSink};
    /// use jsonrpsee_types::ErrorObjectOwned;
    ///
    /// let mut ctx = RpcModule::new(99_usize);
    /// ctx.register_subscription("sub", "notif_name", "unsub", |params, pending, ctx| async move {
    ///     let x = match params.one::<usize>() {
    ///         Ok(x) => x,
    ///         Err(e) => {
    ///             pending.reject(ErrorObjectOwned::from(e)).await;
    ///             // If the subscription has not been "accepted" then
    ///             // the return value will be "ignored" as it's not
    ///             // allowed to send out any further notifications on
    ///             // on the subscription.
    ///             return Ok(());
    ///         }
    ///     };
    ///
    ///     // Mark the subscription is accepted after the params has been parsed successful.
    ///     // This is actually responds the underlying RPC method call and may fail if the
    ///     // connection is closed.
    ///     let sink = pending.accept().await?;
    ///     let sum = x + (*ctx);
    ///
    ///     // This will send out an error notification if it fails.
    ///     //
    ///     // If you need some other behavior implement or custom format of the error field
    ///     // you need to manually handle that.
    ///     let msg = serde_json::value::to_raw_value(&sum).unwrap();
    ///
    ///     // This fails only if the connection is closed
    ///     sink.send(msg).await?;
    ///
    ///     Ok(())
    /// });
    /// ```
    pub fn register_subscription<R, F, Fut>(
        &mut self,
        subscribe_method_name: &'static str,
        notif_method_name: &'static str,
        unsubscribe_method_name: &'static str,
        callback: F,
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        Context: Send + Sync + 'static,
        F: (Fn(Params<'static>, PendingSubscriptionSink, Arc<Context>) -> Fut)
            + Send
            + Sync
            + Clone
            + 'static,
        Fut: Future<Output = R> + Send + 'static,
        R: IntoSubscriptionCloseResponse + Send,
    {
        let subscribers =
            self.verify_and_register_unsubscribe(subscribe_method_name, unsubscribe_method_name)?;
        let ctx = self.ctx.clone();

        // Subscribe
        let callback = {
            self.methods.verify_and_insert(
                subscribe_method_name,
                MethodCallback::Subscription(Arc::new(move |id, params, method_sink, conn| {
                    let uniq_sub = SubscriptionKey {
                        conn_id: conn.conn_id,
                        sub_id: conn.id_provider.next_id(),
                    };

                    // response to the subscription call.
                    let (tx, rx) = oneshot::channel();
                    let (accepted_tx, accepted_rx) = oneshot::channel();

                    let sub_id = uniq_sub.sub_id.clone();
                    let method = notif_method_name;

                    let sink = PendingSubscriptionSink {
                        inner: method_sink.clone(),
                        method: notif_method_name,
                        subscribers: subscribers.clone(),
                        uniq_sub,
                        id: id.clone().into_owned(),
                        subscribe: tx,
                        permit: conn.subscription_permit,
                    };

                    // The subscription callback is a future from the subscription
                    // definition and not the as same when the subscription call has been
                    // completed.
                    //
                    // This runs until the subscription callback has completed.
                    let sub_fut = callback(params.into_owned(), sink, ctx.clone());

                    tokio::spawn(async move {
                        // This will wait for the subscription future to be resolved
                        let response = match futures_util::future::try_join(
                            sub_fut.map(|f| Ok(f)),
                            accepted_rx,
                        )
                        .await
                        {
                            Ok((r, _)) => r.into_response(),
                            // The accept call failed i.e, the subscription was not accepted.
                            Err(_) => return,
                        };

                        match response {
                            SubscriptionCloseResponse::Notif(msg) => {
                                let json =
                                    sub_message_to_json(msg, &notification_prefix(method, &sub_id));
                                let _ = method_sink.send(json).await;
                            }
                            SubscriptionCloseResponse::NotifErr(err) => {
                                let json = sub_err_to_json(err, sub_id, method);
                                let _ = method_sink.send(json).await;
                            }
                            SubscriptionCloseResponse::None => (),
                        }
                    });

                    let id = id.clone().into_owned();

                    Box::pin(async move {
                        match rx.await {
                            Ok(rp) => {
                                // If the subscription was accepted then send a message
                                // to subscription task otherwise rely on the drop impl.
                                if rp.is_success() {
                                    let _ = accepted_tx.send(());
                                }
                                rp
                            }
                            Err(_) => MethodResponse::error(id, ErrorCode::InternalError),
                        }
                    })
                })),
            )?
        };

        Ok(callback)
    }

    /// Similar to [`RpcModule::register_subscription`] but a little lower-level API
    /// where handling the subscription is managed the user i.e, polling the subscription
    /// such as spawning a separate task to do so.
    ///
    /// This is more efficient as this doesn't require cloning the `params` in the subscription
    /// and it won't send out a close message. Such things are delegated to the user of this API
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use jsonrpsee_core::server::{RpcModule, SubscriptionMessage, SubscriptionSink};
    /// use jsonrpsee_types::ErrorObjectOwned;
    ///
    /// let mut ctx = RpcModule::new(99_usize);
    /// ctx.register_subscription_raw("sub", "notif_name", "unsub", |params, pending, ctx| {
    ///     // The params are parsed outside the async block below to avoid cloning the bytes.
    ///     let val = match params.one::<usize>() {
    ///         Ok(val) => val,
    ///         Err(e) => {
    ///             // If the subscription has not been "accepted" then
    ///             // the return value will be "ignored" as it's not
    ///             // allowed to send out any further notifications on
    ///             // on the subscription.
    ///             tokio::spawn(pending.reject(ErrorObjectOwned::from(e)));
    ///             return;
    ///         }
    ///     };
    ///
    ///     tokio::spawn(async move {
    ///         // Mark the subscription is accepted after the params has been parsed successful.
    ///         // This is actually responds the underlying RPC method call and may fail if the
    ///         // connection is closed.
    ///         let sink = pending.accept().await.unwrap();
    ///         let sum = val + (*ctx);
    ///
    ///         let msg = serde_json::value::to_raw_value(&sum).unwrap();
    ///
    ///         // This fails only if the connection is closed
    ///         sink.send(msg).await.unwrap();
    ///     });
    /// });
    /// ```
    pub fn register_subscription_raw<R, F>(
        &mut self,
        subscribe_method_name: &'static str,
        notif_method_name: &'static str,
        unsubscribe_method_name: &'static str,
        callback: F,
    ) -> Result<&mut MethodCallback, RegisterMethodError>
    where
        Context: Send + Sync + 'static,
        F: (Fn(Params, PendingSubscriptionSink, Arc<Context>) -> R) + Send + Sync + Clone + 'static,
        R: IntoSubscriptionCloseResponse,
    {
        let subscribers =
            self.verify_and_register_unsubscribe(subscribe_method_name, unsubscribe_method_name)?;
        let ctx = self.ctx.clone();

        // Subscribe
        let callback = {
            self.methods.verify_and_insert(
                subscribe_method_name,
                MethodCallback::Subscription(Arc::new(move |id, params, method_sink, conn| {
                    let uniq_sub = SubscriptionKey {
                        conn_id: conn.conn_id,
                        sub_id: conn.id_provider.next_id(),
                    };

                    // response to the subscription call.
                    let (tx, rx) = oneshot::channel();

                    let sink = PendingSubscriptionSink {
                        inner: method_sink.clone(),
                        method: notif_method_name,
                        subscribers: subscribers.clone(),
                        uniq_sub,
                        id: id.clone().into_owned(),
                        subscribe: tx,
                        permit: conn.subscription_permit,
                    };

                    callback(params, sink, ctx.clone());

                    let id = id.clone().into_owned();

                    Box::pin(async move {
                        match rx.await {
                            Ok(rp) => rp,
                            Err(_) => MethodResponse::error(id, ErrorCode::InternalError),
                        }
                    })
                })),
            )?
        };

        Ok(callback)
    }

    /// Helper to verify the subscription can be created
    /// and register the unsubscribe handler.
    fn verify_and_register_unsubscribe(
        &mut self,
        subscribe_method_name: &'static str,
        unsubscribe_method_name: &'static str,
    ) -> Result<Subscribers, RegisterMethodError> {
        if subscribe_method_name == unsubscribe_method_name {
            return Err(RegisterMethodError::SubscriptionNameConflict(subscribe_method_name.into()));
        }

        self.methods.verify_method_name(subscribe_method_name)?;
        self.methods.verify_method_name(unsubscribe_method_name)?;

        let subscribers = Subscribers::default();

        // Unsubscribe
        {
            let subscribers = subscribers.clone();
            self.methods.mut_callbacks().insert(
                unsubscribe_method_name,
                MethodCallback::Unsubscription(Arc::new(move |id, params, conn_id, max_response_size| {
                    let sub_id = match params.one::<RpcSubscriptionId>() {
                        Ok(sub_id) => sub_id,
                        Err(_) => {
                            // The params and request id are client input, so only their size is logged.
                            tracing::debug!(
                                target: LOG_TARGET,
                                "Unsubscribe call `{}` failed: couldn't parse subscription id from {} bytes of params",
                                unsubscribe_method_name,
                                params.len_bytes(),
                            );

                            return MethodResponse::response(id, ResponsePayload::success(false), max_response_size);
                        }
                    };

                    let key = SubscriptionKey { conn_id, sub_id: sub_id.into_owned() };
                    let mut subs = subscribers.lock();
                    let removed = subs.remove(&key);
                    let result = removed.is_some();
                    // Dropping the entry's receiver marks the sink inactive, which
                    // `SubscriptionSink::drop` checks under the lock, so drop it before unlocking.
                    drop(removed);
                    drop(subs);

                    if !result {
                        tracing::debug!(
                            target: LOG_TARGET,
                            "Unsubscribe call `{}` subscription key={:?} not an active subscription",
                            unsubscribe_method_name,
                            key,
                        );
                    }

                    MethodResponse::response(id, ResponsePayload::success(result), max_response_size)
                })),
            );
        }

        Ok(subscribers)
    }

    /// Register an alias for an existing_method. Alias uniqueness is enforced.
    pub fn register_alias(
        &mut self,
        alias: &'static str,
        existing_method: &'static str,
    ) -> Result<(), RegisterMethodError> {
        self.methods.verify_method_name(alias)?;

        let callback = match self.methods.callbacks.get(existing_method) {
            Some(callback) => callback.clone(),
            None => return Err(RegisterMethodError::MethodNotFound(existing_method.into())),
        };

        self.methods.mut_callbacks().insert(alias, callback);

        Ok(())
    }
}

fn mock_subscription_permit() -> SubscriptionPermit {
    BoundedSubscriptions::new(1).acquire().expect("1 permit should exist; qed")
}
