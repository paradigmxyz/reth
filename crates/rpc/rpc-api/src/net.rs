use alloy_primitives::U64;
use jsonrpsee::{core::RpcResult, proc_macros::rpc};

/// Net rpc interface.
#[cfg_attr(not(feature = "client"), rpc(server, namespace = "net"))]
#[cfg_attr(feature = "client", rpc(server, client, namespace = "net"))]
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
