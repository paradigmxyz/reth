use alloy_primitives::U64;
use alloy_rpc_macros::rpc;
use alloy_rpc_server::RpcResult;

/// Net rpc interface.
#[rpc(server, namespace = "net")]
pub trait NetApi {
    /// Returns the network ID.
    #[method(name = "version")]
    fn version(&self) -> RpcResult<String>;

    /// Returns number of peers connected to node.
    #[method(name = "peerCount")]
    fn peer_count(&self) -> RpcResult<U64>;

    /// Returns true if client is actively listening for network connections.
    /// Otherwise false.
    #[method(name = "listening")]
    fn is_listening(&self) -> RpcResult<bool>;
}
