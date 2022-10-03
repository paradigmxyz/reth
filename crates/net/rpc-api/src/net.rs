use jsonrpsee::{core::RpcResult as Result, proc_macros::rpc};
use reth_rpc_types::PeerCount;

/// Net rpc interface.
#[rpc(server)]
pub trait NetApi {
    /// Returns protocol version.
    #[method(name = "net_version")]
    fn version(&self) -> Result<String>;

    /// Returns number of peers connected to node.
    #[method(name = "net_peerCount")]
    fn peer_count(&self) -> Result<PeerCount>;

    /// Returns true if client is actively listening for network connections.
    /// Otherwise false.
    #[method(name = "net_listening")]
    fn is_listening(&self) -> Result<bool>;
}
