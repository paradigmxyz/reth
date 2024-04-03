use crate::result::ToRpcResult;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_network_api::{NetworkInfo, PeerKind, Peers};
use reth_primitives::{AnyNode, ChainSpec, NodeRecord};
use reth_rpc_api::AdminApiServer;
use reth_rpc_types::{NodeInfo, PeerEthProtocolInfo, PeerInfo, PeerNetworkInfo, PeerProtocolsInfo};
use std::sync::Arc;

/// `admin` API implementation.
///
/// This type provides the functionality for handling `admin` related requests.
pub struct AdminApi<N> {
    /// An interface to interact with the network
    network: N,
    /// The specification of the blockchain's configuration.
    chain_spec: Arc<ChainSpec>,
}

impl<N> AdminApi<N> {
    /// Creates a new instance of `AdminApi`.
    pub fn new(network: N, chain_spec: Arc<ChainSpec>) -> Self {
        AdminApi { network, chain_spec }
    }
}

#[async_trait]
impl<N> AdminApiServer for AdminApi<N>
where
    N: NetworkInfo + Peers + 'static,
{
    /// Handler for `admin_addPeer`
    fn add_peer(&self, record: NodeRecord) -> RpcResult<bool> {
        self.network.add_peer(record.id, record.tcp_addr());
        Ok(true)
    }

    /// Handler for `admin_removePeer`
    fn remove_peer(&self, record: AnyNode) -> RpcResult<bool> {
        self.network.remove_peer(record.peer_id(), PeerKind::Basic);
        Ok(true)
    }

    /// Handler for `admin_addTrustedPeer`
    fn add_trusted_peer(&self, record: AnyNode) -> RpcResult<bool> {
        if let Some(record) = record.node_record() {
            self.network.add_trusted_peer(record.id, record.tcp_addr())
        }
        self.network.add_trusted_peer_id(record.peer_id());
        Ok(true)
    }

    /// Handler for `admin_removeTrustedPeer`
    fn remove_trusted_peer(&self, record: AnyNode) -> RpcResult<bool> {
        self.network.remove_peer(record.peer_id(), PeerKind::Trusted);
        Ok(true)
    }

    async fn peers(&self) -> RpcResult<Vec<PeerInfo>> {
        let peers = self.network.get_all_peers().await.to_rpc_result()?;
        let peers = peers
            .into_iter()
            .map(|peer| PeerInfo {
                id: Some(peer.remote_id.to_string()),
                name: peer.client_version.to_string(),
                caps: peer.capabilities.capabilities().iter().map(|cap| cap.to_string()).collect(),
                network: PeerNetworkInfo {
                    remote_address: peer.remote_addr.to_string(),
                    local_address: peer
                        .local_addr
                        .unwrap_or_else(|| self.network.local_addr())
                        .to_string(),
                },
                protocols: PeerProtocolsInfo {
                    eth: Some(PeerEthProtocolInfo {
                        difficulty: Some(peer.status.total_difficulty),
                        head: peer.status.blockhash.to_string(),
                        version: peer.status.version as u32,
                    }),
                    pip: None,
                },
            })
            .collect();

        Ok(peers)
    }

    /// Handler for `admin_nodeInfo`
    async fn node_info(&self) -> RpcResult<NodeInfo> {
        let enr = self.network.local_node_record();
        let status = self.network.network_status().await.to_rpc_result()?;
        let config = self.chain_spec.genesis().config.clone();

        Ok(NodeInfo::new(enr, status, config))
    }

    /// Handler for `admin_peerEvents`
    async fn subscribe_peer_events(
        &self,
        _pending: jsonrpsee::PendingSubscriptionSink,
    ) -> jsonrpsee::core::SubscriptionResult {
        Err("admin_peerEvents is not implemented yet".into())
    }
}

impl<N> std::fmt::Debug for AdminApi<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminApi").finish_non_exhaustive()
    }
}
