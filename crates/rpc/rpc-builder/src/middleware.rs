use jsonrpsee::server::middleware::rpc::RpcService;
use tower::Layer;

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
