use jsonrpsee::{
    core::BoxError,
    server::{middleware::rpc::RpcService, HttpRequest, HttpResponse, TowerServiceNoHttp},
};
use tower::{layer::util::Stack, Layer, Service};

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

pub(crate) type HttpStack<M, S> = Stack<M, S>;
pub(crate) type HttpSvc<M, S> = TowerServiceNoHttp<HttpStack<M, S>>;

/// A trait for Tower HTTP middleware that integrates with Reth's RPC server.
///
/// This trait defines Tower middleware that can be applied to HTTP transport layer
/// before requests reach the RPC service. It ensures compatibility with jsonrpsee's
/// server architecture and provides access to the service type produced by the middleware.
///
/// The middleware is applied at the HTTP level and can handle cross-cutting concerns
/// like authentication, compression, rate limiting, and request/response logging.
pub trait RethTowerMiddleware<RpcMiddleware: Layer<RpcService>, S>:
    Layer<
        HttpSvc<RpcMiddleware, S>,
        Service: Service<HttpRequest, Response = HttpResponse, Error = BoxError, Future: Send>
                     + Clone
                     + Send
                     + Sync
                     + 'static,
    > + Clone
    + Send
    + Sync
    + 'static
{
    /// The service type produced by this layer.
    type Service: Service<HttpRequest, Response = HttpResponse, Error = BoxError, Future: Send>
        + Clone
        + Send
        + Sync
        + 'static;
}

impl<T, RpcMiddleware: RethRpcMiddleware, S> RethTowerMiddleware<RpcMiddleware, S> for T
where
    T: Layer<
            HttpSvc<RpcMiddleware, S>,
            Service: Service<
                HttpRequest,
                Response = HttpResponse,
                Error = BoxError,
                Future: Send,
            > + Clone
                         + Send
                         + Sync
                         + 'static,
        > + Clone
        + Send
        + Sync
        + 'static,
{
    type Service = T::Service;
}
