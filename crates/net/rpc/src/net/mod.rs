use jsonrpsee::core::RpcResult as Result;
use reth_network::NetworkHandle;
use reth_rpc_api::NetApiServer;
use reth_rpc_types::PeerCount;

/// `Net` API implementation.
///
/// This type provides the functionality for handling `net_` related requests.
pub struct NetApi {
    /// An interface to interact with the network
    network: NetworkHandle,
    /// The devp2p network ID
    network_id: String,
}

impl std::fmt::Debug for NetApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetApi").field("network_id", &self.network_id).finish_non_exhaustive()
    }
}

/// Net rpc implementation
impl NetApiServer for NetApi {
    fn version(&self) -> Result<String> {
        Ok(self.network_id.clone())
    }

    fn peer_count(&self) -> Result<PeerCount> {
        Ok(PeerCount::Hex(self.network.num_connected_peers().into()))
    }

    fn is_listening(&self) -> Result<bool> {
        Ok(true)
    }
}
