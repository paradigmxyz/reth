use std::sync::Arc;

use alloy_genesis::ChainConfig;
use alloy_rpc_types_admin::{
    EthInfo, EthPeerInfo, EthProtocolInfo, NodeInfo, PeerInfo, PeerNetworkInfo, PeerProtocolInfo,
    Ports, ProtocolInfo,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::{EthChainSpec, EthereumHardforks, ForkCondition};
use reth_network_api::{NetworkInfo, Peers};
use reth_network_peers::{id2pk, AnyNode, NodeRecord};
use reth_network_types::PeerKind;
use reth_primitives::EthereumHardfork;
use reth_rpc_api::AdminApiServer;
use reth_rpc_server_types::ToRpcResult;

/// `admin` API implementation.
///
/// This type provides the functionality for handling `admin` related requests.
pub struct AdminApi<N, ChainSpec> {
    /// An interface to interact with the network
    network: N,
    /// The specification of the blockchain's configuration.
    chain_spec: Arc<ChainSpec>,
}

impl<N, ChainSpec> AdminApi<N, ChainSpec> {
    /// Creates a new instance of `AdminApi`.
    pub const fn new(network: N, chain_spec: Arc<ChainSpec>) -> Self {
        Self { network, chain_spec }
    }
}

#[async_trait]
impl<N, ChainSpec> AdminApiServer for AdminApi<N, ChainSpec>
where
    N: NetworkInfo + Peers + 'static,
    ChainSpec: EthChainSpec + EthereumHardforks + Send + Sync + 'static,
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
        let mut config = ChainConfig {
            chain_id: self.chain_spec.chain().id(),
            terminal_total_difficulty_passed: self
                .chain_spec
                .get_final_paris_total_difficulty()
                .is_some(),
            terminal_total_difficulty: self.chain_spec.fork(EthereumHardfork::Paris).ttd(),
            ..self.chain_spec.genesis().config.clone()
        };

        // helper macro to set the block or time for a hardfork if known
        macro_rules! set_block_or_time {
            ($config:expr, [$( $field:ident => $fork:ident,)*]) => {
                $(
                    // don't overwrite if already set
                    if $config.$field.is_none() {
                        $config.$field = match self.chain_spec.fork(EthereumHardfork::$fork) {
                            ForkCondition::Block(block) => Some(block),
                            ForkCondition::TTD { fork_block, .. } => fork_block,
                            ForkCondition::Timestamp(ts) => Some(ts),
                            ForkCondition::Never => None,
                        };
                    }
                )*
            };
        }

        set_block_or_time!(config, [
            homestead_block => Homestead,
            dao_fork_block => Dao,
            eip150_block => Tangerine,
            eip155_block => SpuriousDragon,
            eip158_block => SpuriousDragon,
            byzantium_block => Byzantium,
            constantinople_block => Constantinople,
            petersburg_block => Petersburg,
            istanbul_block => Istanbul,
            muir_glacier_block => MuirGlacier,
            berlin_block => Berlin,
            london_block => London,
            arrow_glacier_block => ArrowGlacier,
            gray_glacier_block => GrayGlacier,
            shanghai_time => Shanghai,
            cancun_time => Cancun,
            prague_time => Prague,
        ]);

        Ok(NodeInfo {
            id: id2pk(enode.id)
                .map(|pk| pk.to_string())
                .unwrap_or_else(|_| alloy_primitives::hex::encode(enode.id.as_slice())),
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
        })
    }

    /// Handler for `admin_peerEvents`
    async fn subscribe_peer_events(
        &self,
        _pending: jsonrpsee::PendingSubscriptionSink,
    ) -> jsonrpsee::core::SubscriptionResult {
        Err("admin_peerEvents is not implemented yet".into())
    }
}

impl<N, ChainSpec> std::fmt::Debug for AdminApi<N, ChainSpec> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminApi").finish_non_exhaustive()
    }
}
