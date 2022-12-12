use crate::eth::EthApiSpec;
use jsonrpsee::core::RpcResult as Result;
use reth_network::NetworkHandle;
use reth_rpc_api::NetApiServer;
use reth_rpc_types::PeerCount;

/// `Net` API implementation.
///
/// This type provides the functionality for handling `net` related requests.
pub struct NetApi {
    /// An interface to interact with the network
    network: NetworkHandle,
    /// The implementation of `eth` API
    eth: Box<dyn EthApiSpec>,
}

impl std::fmt::Debug for NetApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetApi").finish_non_exhaustive()
    }
}

/// Net rpc implementation
impl NetApiServer for NetApi {
    fn version(&self) -> Result<String> {
        Ok(self.eth.chain_id().to_string())
    }

    fn peer_count(&self) -> Result<PeerCount> {
        Ok(PeerCount::Hex(self.network.num_connected_peers().into()))
    }

    fn is_listening(&self) -> Result<bool> {
        Ok(true)
    }
}
