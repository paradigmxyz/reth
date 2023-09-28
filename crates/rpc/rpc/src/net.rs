use crate::eth::EthApiSpec;
use jsonrpsee::core::RpcResult as Result;
use reth_network_api::PeersInfo;
use reth_primitives::U64;
use reth_rpc_api::NetApiServer;
use reth_rpc_types::PeerCount;

/// `Net` API implementation.
///
/// This type provides the functionality for handling `net` related requests.
pub struct NetApi<Net, Eth> {
    /// An interface to interact with the network
    network: Net,
    /// The implementation of `eth` API
    eth: Eth,
}

// === impl NetApi ===

impl<Net, Eth> NetApi<Net, Eth> {
    /// Returns a new instance with the given network and eth interface implementations
    pub fn new(network: Net, eth: Eth) -> Self {
        Self { network, eth }
    }
}

/// Net rpc implementation
impl<Net, Eth> NetApiServer for NetApi<Net, Eth>
where
    Net: PeersInfo + 'static,
    Eth: EthApiSpec + 'static,
{
    /// Handler for `net_version`
    fn version(&self) -> Result<String> {
        Ok(self.eth.chain_id().to_string())
    }

    /// Handler for `net_peerCount`
    fn peer_count(&self) -> Result<PeerCount> {
        Ok(PeerCount::Hex(U64::from(self.network.num_connected_peers())))
    }

    /// Handler for `net_listening`
    fn is_listening(&self) -> Result<bool> {
        Ok(true)
    }
}

impl<Net, Eth> std::fmt::Debug for NetApi<Net, Eth> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetApi").finish_non_exhaustive()
    }
}
