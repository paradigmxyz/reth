use jsonrpsee::server::{
    middleware::rpc::RpcService, HttpRequest, HttpResponse, TowerServiceNoHttp,
};
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
