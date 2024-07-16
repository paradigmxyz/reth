use std::sync::Arc;

use alloy_genesis::ChainConfig;
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::ChainSpec;
use reth_network_api::{NetworkInfo, PeerKind, Peers};
use reth_network_peers::{id2pk, AnyNode, NodeRecord};
use reth_rpc_api::AdminApiServer;
use reth_rpc_server_types::ToRpcResult;
use reth_rpc_types::admin::{
    EthInfo, EthPeerInfo, EthProtocolInfo, NodeInfo, PeerInfo, PeerNetworkInfo, PeerProtocolInfo,
    Ports, ProtocolInfo,
};

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
    pub const fn new(network: N, chain_spec: Arc<ChainSpec>) -> Self {
        Self { network, chain_spec }
    }
}

#[async_trait]
impl<N> AdminApiServer for AdminApi<N>
where
    N: NetworkInfo + Peers + 'static,
{
    /// Handler for `admin_addPeer`
    fn add_peer(&self, record: NodeRecord) -> RpcResult<bool> {
        self.network.add_peer_with_udp(record.id, record.tcp_addr(), record.udp_addr());
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
            self.network.add_trusted_peer_with_udp(record.id, record.tcp_addr(), record.udp_addr())
        }
        self.network.add_trusted_peer_id(record.peer_id());
        Ok(true)
    }

    /// Handler for `admin_removeTrustedPeer`
    fn remove_trusted_peer(&self, record: AnyNode) -> RpcResult<bool> {
        self.network.remove_peer(record.peer_id(), PeerKind::Trusted);
        Ok(true)
    }

    /// Handler for `admin_peers`
    async fn peers(&self) -> RpcResult<Vec<PeerInfo>> {
        let peers = self.network.get_all_peers().await.to_rpc_result()?;
        let mut infos = Vec::with_capacity(peers.len());

        for peer in peers {
            if let Ok(pk) = id2pk(peer.remote_id) {
                infos.push(PeerInfo {
                    id: pk.to_string(),
                    name: peer.client_version.to_string(),
                    enode: peer.enode,
                    enr: peer.enr,
                    caps: peer
                        .capabilities
                        .capabilities()
                        .iter()
                        .map(|cap| cap.to_string())
                        .collect(),
                    network: PeerNetworkInfo {
                        remote_address: peer.remote_addr,
                        local_address: peer.local_addr.unwrap_or_else(|| self.network.local_addr()),
                        inbound: peer.direction.is_incoming(),
                        trusted: peer.kind.is_trusted(),
                        static_node: peer.kind.is_static(),
                    },
                    protocols: PeerProtocolInfo {
                        eth: Some(EthPeerInfo::Info(EthInfo {
                            version: peer.status.version as u64,
                        })),
                        snap: None,
                        other: Default::default(),
                    },
                })
            }
        }

        Ok(infos)
    }

    /// Handler for `admin_nodeInfo`
    async fn node_info(&self) -> RpcResult<NodeInfo> {
        let enode = self.network.local_node_record();
        let status = self.network.network_status().await.to_rpc_result()?;
        let config = ChainConfig {
            chain_id: self.chain_spec.chain.id(),
            terminal_total_difficulty_passed: self
                .chain_spec
                .get_final_paris_total_difficulty()
                .is_some(),
            ..self.chain_spec.genesis().config.clone()
        };

        let node_info = NodeInfo {
            id: enode.id.to_string(),
            name: status.client_version,
            enode: enode.to_string(),
            enr: self.network.local_enr().to_string(),
            ip: enode.address,
            ports: Ports { discovery: enode.udp_port, listener: enode.tcp_port },
            listen_addr: enode.tcp_addr(),
            protocols: ProtocolInfo {
                eth: Some(EthProtocolInfo {
                    network: status.eth_protocol_info.network,
                    difficulty: status.eth_protocol_info.difficulty,
                    genesis: status.eth_protocol_info.genesis,
                    config,
                    head: status.eth_protocol_info.head,
                }),
                snap: None,
            },
        };

        Ok(node_info)
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
