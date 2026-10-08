use crate::{
    module::Callback, subscription::Connection, ErrorCode, Id, MethodResponse, Notification,
    Request, RpcModule, SubscriptionId,
};
use futures_util::future::BoxFuture;
use std::{
    future::Future,
    mem,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::oneshot;
use tower::{
    layer::{
        util::{Identity, Stack},
        LayerFn,
    },
    util::Either,
    Layer, ServiceBuilder,
};

/// A JSON-RPC service, which middleware wraps to intercept calls.
pub trait RpcServiceT: Send + Sync {
    /// Processes a method call.
    fn call(&self, req: Request) -> impl Future<Output = MethodResponse> + Send;

    /// Processes the calls of a batch.
    ///
    /// Calls [`Self::call`] for every request by default.
    fn batch(&self, reqs: Vec<Request>) -> impl Future<Output = Vec<MethodResponse>> + Send {
        futures_util::future::join_all(reqs.into_iter().map(|req| self.call(req)))
    }

    /// Processes a notification, which gets no response.
    ///
    /// Does nothing by default. Methods are not called for notifications.
    fn notification(&self, n: Notification) -> impl Future<Output = ()> + Send {
        drop(n);
        std::future::ready(())
    }
}

impl<A: RpcServiceT, B: RpcServiceT> RpcServiceT for Either<A, B> {
    async fn call(&self, req: Request) -> MethodResponse {
        match self {
            Self::Left(svc) => svc.call(req).await,
            Self::Right(svc) => svc.call(req).await,
        }
    }

    async fn batch(&self, reqs: Vec<Request>) -> Vec<MethodResponse> {
        match self {
            Self::Left(svc) => svc.batch(reqs).await,
            Self::Right(svc) => svc.batch(reqs).await,
        }
    }

    async fn notification(&self, n: Notification) {
        match self {
            Self::Left(svc) => svc.notification(n).await,
            Self::Right(svc) => svc.notification(n).await,
        }
    }
}

impl<T: RpcServiceT + ?Sized> RpcServiceT for Arc<T> {
    fn call(&self, req: Request) -> impl Future<Output = MethodResponse> + Send {
        (**self).call(req)
    }

    fn batch(&self, reqs: Vec<Request>) -> impl Future<Output = Vec<MethodResponse>> + Send {
        (**self).batch(reqs)
    }

    fn notification(&self, n: Notification) -> impl Future<Output = ()> + Send {
        (**self).notification(n)
    }
}

/// The innermost service, which dispatches calls to the registered methods.
///
/// Panics in methods are caught and answered with an internal error.
#[derive(Clone, Debug)]
pub struct RpcService {
    methods: RpcModule,
    max_response_size: usize,
    conn: Option<Arc<Connection>>,
}

impl RpcService {
    pub(crate) const fn new(
        methods: RpcModule,
        max_response_size: usize,
        conn: Option<Arc<Connection>>,
    ) -> Self {
        Self { methods, max_response_size, conn }
    }
}

impl RpcServiceT for RpcService {
    fn call(&self, req: Request) -> impl Future<Output = MethodResponse> + Send {
        let Request { id, method, params, extensions } = req;
        let max_size = self.max_response_size;
        let Some(callback) = self.methods.method(&method) else {
            return ResponseFuture::ready(MethodResponse::error(id, ErrorCode::MethodNotFound))
        };
        let internal_error =
            |id| ResponseFuture::ready(MethodResponse::error(id, ErrorCode::InternalError));
        match callback {
            Callback::Sync(callback) => {
                match catch_unwind(AssertUnwindSafe(|| {
                    callback(id.clone(), params, &extensions, max_size)
                })) {
                    Ok(response) => ResponseFuture::ready(response),
                    Err(_) => internal_error(id),
                }
            }
            Callback::Async(callback) => {
                match catch_unwind(AssertUnwindSafe(|| {
                    callback(id.clone(), params, extensions, max_size)
                })) {
                    Ok(fut) => ResponseFuture(Inner::Future { fut, id }),
                    Err(_) => internal_error(id),
                }
            }
            Callback::Subscription { notification, unsubscribe, callback } => {
                let Some(conn) = &self.conn else { return internal_error(id) };
                match conn.subscribe(id.clone(), notification, unsubscribe) {
                    Ok((pending, rx)) => {
                        match catch_unwind(AssertUnwindSafe(|| {
                            callback(params, pending, extensions)
                        })) {
                            Ok(()) => ResponseFuture(Inner::Subscription { rx, id }),
                            Err(_) => internal_error(id),
                        }
                    }
                    Err(err) => ResponseFuture::ready(MethodResponse::error(id, err)),
                }
            }
            Callback::Unsubscription { method } => {
                let Some(conn) = &self.conn else { return internal_error(id) };
                let closed = params
                    .one::<SubscriptionId>()
                    .is_ok_and(|sub_id| conn.unsubscribe(method, &sub_id));
                ResponseFuture::ready(MethodResponse::response(id, &closed, max_size))
            }
        }
    }
}

/// Future returned by [`RpcService`].
struct ResponseFuture(Inner);

enum Inner {
    Ready(Option<MethodResponse>),
    Future { fut: BoxFuture<'static, MethodResponse>, id: Id },
    Subscription { rx: oneshot::Receiver<MethodResponse>, id: Id },
}

impl ResponseFuture {
    const fn ready(response: MethodResponse) -> Self {
        Self(Inner::Ready(Some(response)))
    }
}

impl Future for ResponseFuture {
    type Output = MethodResponse;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<MethodResponse> {
        let internal_error =
            |id: &mut Id| MethodResponse::error(mem::take(id), ErrorCode::InternalError);
        match &mut self.0 {
            Inner::Ready(response) => {
                Poll::Ready(response.take().expect("polled after completion"))
            }
            Inner::Future { fut, id } => {
                match catch_unwind(AssertUnwindSafe(|| fut.as_mut().poll(cx))) {
                    Ok(poll) => poll,
                    Err(_) => Poll::Ready(internal_error(id)),
                }
            }
            // The sender is dropped if the subscription was neither accepted nor rejected.
            Inner::Subscription { rx, id } => {
                Pin::new(rx).poll(cx).map(|res| res.unwrap_or_else(|_| internal_error(id)))
            }
        }
    }
}

/// Builds the middleware stack around [`RpcService`].
#[derive(Clone, Debug)]
pub struct RpcServiceBuilder<L>(ServiceBuilder<L>);

impl RpcServiceBuilder<Identity> {
    /// Creates a builder without middleware.
    pub const fn new() -> Self {
        Self(ServiceBuilder::new())
    }
}

impl Default for RpcServiceBuilder<Identity> {
    fn default() -> Self {
        Self::new()
    }
}

impl<L> RpcServiceBuilder<L> {
    /// Adds a middleware layer.
    pub fn layer<T>(self, layer: T) -> RpcServiceBuilder<Stack<T, L>> {
        RpcServiceBuilder(self.0.layer(layer))
    }

    /// Adds a middleware layer if `layer` is `Some`.
    pub fn option_layer<T>(
        self,
        layer: Option<T>,
    ) -> RpcServiceBuilder<Stack<Either<T, Identity>, L>> {
        RpcServiceBuilder(self.0.option_layer(layer))
    }

    /// Adds a middleware layer created from a function.
    pub fn layer_fn<F>(self, f: F) -> RpcServiceBuilder<Stack<LayerFn<F>, L>> {
        RpcServiceBuilder(self.0.layer_fn(f))
    }

    /// Adds an [`RpcLogger`], which traces every call and its response, truncated to
    /// `max_log_len` bytes.
    pub fn rpc_logger(self, max_log_len: u32) -> RpcServiceBuilder<Stack<RpcLoggerLayer, L>> {
        self.layer(RpcLoggerLayer::new(max_log_len))
    }

    /// Wraps `service` in the middleware layers.
    pub fn service<S>(&self, service: S) -> L::Service
    where
        L: Layer<S>,
    {
        self.0.service(service)
    }
}

/// Layer that wraps services in an [`RpcLogger`].
#[derive(Clone, Copy, Debug)]
pub struct RpcLoggerLayer(usize);

impl RpcLoggerLayer {
    /// Creates a layer that truncates logged messages to `max_log_len` bytes.
    pub const fn new(max_log_len: u32) -> Self {
        Self(max_log_len as usize)
    }
}

impl<S> Layer<S> for RpcLoggerLayer {
    type Service = RpcLogger<S>;

    fn layer(&self, service: S) -> Self::Service {
        RpcLogger { service, max_log_len: self.0 }
    }
}

/// Middleware that traces calls, notifications and responses.
#[derive(Clone, Debug)]
pub struct RpcLogger<S> {
    service: S,
    max_log_len: usize,
}

impl<S: RpcServiceT> RpcServiceT for RpcLogger<S> {
    fn call(&self, req: Request) -> impl Future<Output = MethodResponse> + Send {
        let max = self.max_log_len;
        tracing::trace!(
            target: "rpc::jsonrpc",
            id = ?req.id,
            method = %req.method,
            params = truncate(req.params.as_str().unwrap_or_default(), max),
            "call"
        );
        let fut = self.service.call(req);
        async move {
            let response = fut.await;
            tracing::trace!(target: "rpc::jsonrpc", response = truncate(response.as_json(), max));
            response
        }
    }

    fn batch(&self, reqs: Vec<Request>) -> impl Future<Output = Vec<MethodResponse>> + Send {
        tracing::trace!(target: "rpc::jsonrpc", len = reqs.len(), "batch");
        futures_util::future::join_all(reqs.into_iter().map(|req| self.call(req)))
    }

    fn notification(&self, n: Notification) -> impl Future<Output = ()> + Send {
        tracing::trace!(
            target: "rpc::jsonrpc",
            method = %n.method,
            params = truncate(n.params.as_str().unwrap_or_default(), self.max_log_len),
            "notification"
        );
        self.service.notification(n)
    }
}

fn truncate(s: &str, max: usize) -> &str {
    &s[..s.floor_char_boundary(max)]
}
