use reth_jasonrpeesea::{
    server::{HttpRequest, HttpResponse, TowerService},
    RpcService, RpcServiceT,
};
use tower::{
    layer::util::{Identity, Stack},
    Layer,
};

/// A Helper alias trait for the RPC middleware supported by the server.
pub trait RethRpcMiddleware:
    Layer<RpcService, Service: RpcServiceT + Send + Sync + Clone + 'static>
    + Clone
    + Send
    + Sync
    + 'static
{
}

impl<T> RethRpcMiddleware for T where
    T: Layer<RpcService, Service: RpcServiceT + Send + Sync + Clone + 'static>
        + Clone
        + Send
        + Sync
        + 'static
{
}

/// Inner HTTP transport service type for auth-server middleware.
pub type AuthHttpService<RM> = TowerService<Stack<RM, Identity>>;

/// Helper alias trait for auth-server HTTP transport middleware layers.
pub trait RethAuthHttpMiddleware<RM: Layer<RpcService>>:
    tower::Layer<
        AuthHttpService<RM>,
        Service: tower::Service<
            HttpRequest,
            Response = HttpResponse,
            Error = tower::BoxError,
            Future: Send,
        > + Send
                     + Clone
                     + 'static,
    > + Clone
    + Send
    + 'static
{
}

impl<T, RM: Layer<RpcService>> RethAuthHttpMiddleware<RM> for T where
    T: tower::Layer<
            AuthHttpService<RM>,
            Service: tower::Service<
                HttpRequest,
                Response = HttpResponse,
                Error = tower::BoxError,
                Future: Send,
            > + Send
                         + Clone
                         + 'static,
        > + Clone
        + Send
        + 'static
{
}
