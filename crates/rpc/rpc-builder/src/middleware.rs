use jsonrpsee::{
    core::middleware::{Batch, BatchEntry, Notification},
    server::{
        middleware::rpc::{RpcService, RpcServiceT},
        HttpRequest, HttpResponse, TowerServiceNoHttp,
    },
    types::Request,
    MethodResponse,
};
use reth_rpc_server_types::BatchResponseCommit;
use std::future::Future;
use tower::{
    layer::util::{Identity, Stack},
    Layer,
};

/// A Helper alias trait for the RPC middleware supported by the server.
pub trait RethRpcMiddleware:
    Layer<
        RpcService,
        Service: jsonrpsee::server::middleware::rpc::RpcServiceT<
            MethodResponse = jsonrpsee::MethodResponse,
            BatchResponse = jsonrpsee::MethodResponse,
            NotificationResponse = jsonrpsee::MethodResponse,
        > + Send
                     + Sync
                     + Clone
                     + 'static,
    > + Clone
    + Send
    + 'static
{
}

impl<T> RethRpcMiddleware for T where
    T: Layer<
            RpcService,
            Service: jsonrpsee::server::middleware::rpc::RpcServiceT<
                MethodResponse = jsonrpsee::MethodResponse,
                BatchResponse = jsonrpsee::MethodResponse,
                NotificationResponse = jsonrpsee::MethodResponse,
            > + Send
                         + Sync
                         + Clone
                         + 'static,
        > + Clone
        + Send
        + 'static
{
}

/// Inner HTTP transport service type for auth-server middleware.
pub type AuthHttpService<RM> = TowerServiceNoHttp<Stack<RM, Identity>>;

/// Helper alias trait for auth-server HTTP transport middleware layers.
pub trait RethAuthHttpMiddleware<RM>:
    tower::Layer<
        AuthHttpService<RM>,
        Service: tower::Service<
            HttpRequest,
            Response = HttpResponse,
            Error = tower::BoxError,
            Future: Send,
        > + Send
                     + Clone,
    > + Clone
    + Send
    + 'static
{
}

impl<T, RM> RethAuthHttpMiddleware<RM> for T where
    T: tower::Layer<
            AuthHttpService<RM>,
            Service: tower::Service<
                HttpRequest,
                Response = HttpResponse,
                Error = tower::BoxError,
                Future: Send,
            > + Send
                         + Clone,
        > + Clone
        + Send
        + 'static
{
}

/// Defers participating handlers' state updates until the complete batch response fits.
#[derive(Clone, Debug)]
pub(crate) struct RpcBatchResponseService<S> {
    inner: S,
}

impl<S> RpcBatchResponseService<S> {
    pub(crate) const fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S> RpcServiceT for RpcBatchResponseService<S>
where
    S: RpcServiceT<BatchResponse = MethodResponse> + Send + Sync + Clone + 'static,
{
    type MethodResponse = S::MethodResponse;
    type BatchResponse = MethodResponse;
    type NotificationResponse = S::NotificationResponse;

    fn call<'a>(&self, req: Request<'a>) -> impl Future<Output = Self::MethodResponse> + Send + 'a {
        self.inner.call(req)
    }

    fn batch<'a>(
        &self,
        mut batch: Batch<'a>,
    ) -> impl Future<Output = Self::BatchResponse> + Send + 'a {
        let commit = BatchResponseCommit::default();
        for entry in batch.iter_mut().flatten() {
            if let BatchEntry::Call(request) = entry {
                request.extensions_mut().insert(commit.clone());
            }
        }
        let response = self.inner.batch(batch);
        async move {
            let response = response.await;
            if response.is_batch() {
                commit.commit();
            }
            response
        }
    }

    fn notification<'a>(
        &self,
        notification: Notification<'a>,
    ) -> impl Future<Output = Self::NotificationResponse> + Send + 'a {
        self.inner.notification(notification)
    }
}
