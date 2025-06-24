use crate::RpcRequestMetricsService;
use jsonrpsee::server::middleware::rpc::RpcService;
use tower::Layer;

/// A Helper alias trait for the RPC middleware supported by the server.
///
/// Note: This trait is only used to simplify the trait bounds in the RPC builder stack.
pub trait RethRpcMiddleware:
    Layer<
        RpcRequestMetricsService<RpcService>,
        Service: jsonrpsee::server::middleware::rpc::RpcServiceT<
            MethodResponse = jsonrpsee::MethodResponse,
            BatchResponse = jsonrpsee::MethodResponse,
            NotificationResponse = jsonrpsee::MethodResponse,
        > + Send
                     + Sync
                     + 'static,
    > + Clone
    + Send
    + 'static
{
}

impl<T> RethRpcMiddleware for T where
    T: Layer<
            RpcRequestMetricsService<RpcService>,
            Service: jsonrpsee::server::middleware::rpc::RpcServiceT<
                MethodResponse = jsonrpsee::MethodResponse,
                BatchResponse = jsonrpsee::MethodResponse,
                NotificationResponse = jsonrpsee::MethodResponse,
            > + Send
                         + Sync
                         + 'static,
        > + Clone
        + Send
        + 'static
{
}
