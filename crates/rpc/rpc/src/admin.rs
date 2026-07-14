use std::{sync::Arc, time::Duration};

use alloy_genesis::ChainConfig;
use alloy_primitives::keccak256;
use alloy_rpc_types_admin::{
    EthInfo, EthPeerInfo, EthProtocolInfo, NodeInfo, PeerInfo, PeerNetworkInfo, PeerProtocolInfo,
    Ports, ProtocolInfo,
};
use async_trait::async_trait;
use jsonrpsee::core::RpcResult;
use reth_chainspec::{EthChainSpec, EthereumHardfork, EthereumHardforks, ForkCondition};
use reth_network_api::{NetworkInfo, Peers};
use reth_network_peers::{AnyNode, NodeRecord};
use reth_network_types::PeerKind;
use reth_rpc_api::{AdminApiServer, TracingDirectivesRequest, TracingDirectivesResponse};
use reth_rpc_server_types::{
    result::{internal_rpc_err, invalid_params_rpc_err},
    ToRpcResult,
};
use reth_transaction_pool::TransactionPool;
use tokio::{sync::Mutex, task::JoinHandle};
use tracing::{info, warn};

/// Maximum duration of a tracing override.
const MAX_TRACING_TTL_SECS: u64 = 3600;
/// Maximum accepted tracing directive length.
const MAX_TRACING_DIRECTIVES_LEN: usize = 1024;

#[derive(Debug, Default)]
struct TracingRevertState {
    generation: u64,
    task: Option<JoinHandle<()>>,
}

static TRACING_REVERT: Mutex<TracingRevertState> =
    Mutex::const_new(TracingRevertState { generation: 0, task: None });

/// `admin` API implementation.
///
/// This type provides the functionality for handling `admin` related requests.
pub struct AdminApi<N, ChainSpec, Pool> {
    /// An interface to interact with the network
    network: N,
    /// The specification of the blockchain's configuration.
    chain_spec: Arc<ChainSpec>,
    /// The transaction pool
    pool: Pool,
}

impl<N, ChainSpec, Pool> AdminApi<N, ChainSpec, Pool> {
    /// Creates a new instance of `AdminApi`.
    pub const fn new(network: N, chain_spec: Arc<ChainSpec>, pool: Pool) -> Self {
        Self { network, chain_spec, pool }
    }
}

#[async_trait]
impl<N, ChainSpec, Pool> AdminApiServer for AdminApi<N, ChainSpec, Pool>
where
    N: NetworkInfo + Peers + 'static,
    ChainSpec: EthChainSpec + EthereumHardforks + Send + Sync + 'static,
    Pool: TransactionPool + 'static,
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
        if let Some(trusted) = record.trusted_peer().cloned() {
            self.network.add_trusted_peer_node(trusted);
        } else {
            if let Some(record) = record.node_record() {
                self.network.add_trusted_peer_with_udp(
                    record.id,
                    record.tcp_addr(),
                    record.udp_addr(),
                )
            }
            self.network.add_trusted_peer_id(record.peer_id());
        }
        Ok(true)
    }

    /// Handler for `admin_removeTrustedPeer`
    fn remove_trusted_peer(&self, record: AnyNode) -> RpcResult<bool> {
        self.network.remove_peer(record.peer_id(), PeerKind::Trusted);
        Ok(true)
    }

    /// Handler for `admin_banPeer`
    fn ban_peer(&self, record: AnyNode) -> RpcResult<bool> {
        self.network.ban_peer(record.peer_id());
        Ok(true)
    }

    /// Handler for `admin_unbanPeer`
    fn unban_peer(&self, record: AnyNode) -> RpcResult<bool> {
        self.network.unban_peer(record.peer_id());
        Ok(true)
    }

    /// Handler for `admin_peers`
    async fn peers(&self) -> RpcResult<Vec<PeerInfo>> {
        let peers = self.network.get_all_peers().await.to_rpc_result()?;
        let mut infos = Vec::with_capacity(peers.len());

        for peer in peers {
            infos.push(PeerInfo {
                id: alloy_primitives::hex::encode(keccak256(peer.remote_id.as_slice())),
                name: peer.client_version.to_string(),
                enode: peer.enode,
                enr: peer.enr,
                caps: peer.capabilities.capabilities().iter().map(|cap| cap.to_string()).collect(),
                network: PeerNetworkInfo {
                    remote_address: peer.remote_addr,
                    local_address: peer.local_addr.unwrap_or_else(|| self.network.local_addr()),
                    inbound: peer.direction.is_incoming(),
                    trusted: peer.kind.is_trusted(),
                    static_node: peer.kind.is_static(),
                },
                protocols: PeerProtocolInfo {
                    eth: Some(EthPeerInfo::Info(EthInfo { version: peer.status.version as u64 })),
                    snap: None,
                    other: Default::default(),
                },
            })
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
                .final_paris_total_difficulty()
                .is_some(),
            terminal_total_difficulty: self
                .chain_spec
                .ethereum_fork_activation(EthereumHardfork::Paris)
                .ttd(),
            deposit_contract_address: self.chain_spec.deposit_contract().map(|dc| dc.address),
            ..self.chain_spec.genesis().config.clone()
        };

        // helper macro to set the block or time for a hardfork if known
        macro_rules! set_block_or_time {
            ($config:expr, [$( $field:ident => $fork:ident,)*]) => {
                $(
                    // don't overwrite if already set
                    if $config.$field.is_none() {
                        $config.$field = match self.chain_spec.ethereum_fork_activation(EthereumHardfork::$fork) {
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
            id: alloy_primitives::hex::encode(keccak256(enode.id.as_slice())),
            name: status.client_version,
            enode: enode.to_string(),
            enr: self.network.local_enr().to_string(),
            ip: enode.address,
            ports: Ports { discovery: enode.udp_port, listener: enode.tcp_port },
            listen_addr: enode.tcp_addr(),
            #[expect(deprecated)]
            protocols: ProtocolInfo {
                eth: Some(EthProtocolInfo {
                    network: status.eth_protocol_info.network,
                    genesis: status.eth_protocol_info.genesis,
                    config,
                    head: status.eth_protocol_info.head,
                    difficulty: None,
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

    /// Handler for `admin_clearTxpool`
    async fn clear_txpool(&self) -> RpcResult<u64> {
        let all_hashes = self.pool.all_transaction_hashes();
        let count = all_hashes.len() as u64;
        let _ = self.pool.remove_transactions(all_hashes);
        Ok(count)
    }

    /// Handler for `admin_tracingDirectives`.
    async fn tracing_directives(
        &self,
        request: TracingDirectivesRequest,
    ) -> RpcResult<TracingDirectivesResponse> {
        validate_tracing_directives(&request)?;
        if !reth_tracing::log_handle_available() {
            return Err(internal_rpc_err(
                "tracing reload is not active; enable the admin RPC namespace at startup",
            ));
        }

        let baseline = reth_tracing::startup_log_directives().unwrap_or_default().to_string();
        let ttl_secs = request.ttl_secs;
        let mut state = TRACING_REVERT.lock().await;

        let directives = if ttl_secs == Some(0) {
            reth_tracing::set_log_vmodule(&baseline).map_err(internal_rpc_err)?;
            baseline.clone()
        } else {
            let directives = request.directives.trim().to_string();
            reth_tracing::set_log_vmodule(&directives).map_err(invalid_params_rpc_err)?;
            directives
        };

        if let Some(previous) = state.task.take() {
            previous.abort();
        }
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;

        if let Some(ttl_secs @ 1..=MAX_TRACING_TTL_SECS) = ttl_secs {
            let revert_baseline = baseline.clone();
            state.task = Some(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(ttl_secs)).await;
                let mut state = TRACING_REVERT.lock().await;
                if state.generation != generation {
                    return;
                }
                match reth_tracing::set_log_vmodule(&revert_baseline) {
                    Ok(()) => {
                        info!(target: "reth::rpc::admin", %revert_baseline, "Reverted tracing directives after TTL")
                    }
                    Err(err) => {
                        warn!(target: "reth::rpc::admin", %err, "Failed to revert tracing directives after TTL")
                    }
                }
                state.task = None;
            }));
        }
        drop(state);

        info!(target: "reth::rpc::admin", %directives, ?ttl_secs, "Applied tracing directives");
        Ok(TracingDirectivesResponse { applied: directives, ttl_secs, reverts_to: baseline })
    }
}

fn validate_tracing_directives(request: &TracingDirectivesRequest) -> RpcResult<()> {
    if request.ttl_secs.is_some_and(|ttl| ttl > MAX_TRACING_TTL_SECS) {
        return Err(invalid_params_rpc_err(format!(
            "ttlSecs must be in 0..={MAX_TRACING_TTL_SECS}"
        )));
    }
    if request.ttl_secs == Some(0) {
        return Ok(())
    }
    let directives = request.directives.trim();
    if directives.is_empty() {
        return Err(invalid_params_rpc_err("directives must not be empty"));
    }
    if directives.len() > MAX_TRACING_DIRECTIVES_LEN {
        return Err(invalid_params_rpc_err(format!(
            "directives must be at most {MAX_TRACING_DIRECTIVES_LEN} characters"
        )));
    }
    Ok(())
}

impl<N, ChainSpec, Pool> std::fmt::Debug for AdminApi<N, ChainSpec, Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminApi").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_tracing_directives() {
        let request = TracingDirectivesRequest {
            directives: "  info,reth::net=trace  ".to_string(),
            ttl_secs: Some(30),
        };
        validate_tracing_directives(&request).unwrap();
        validate_tracing_directives(&TracingDirectivesRequest {
            directives: "trace".to_string(),
            ttl_secs: None,
        })
        .unwrap();
        validate_tracing_directives(&TracingDirectivesRequest {
            directives: String::new(),
            ttl_secs: Some(0),
        })
        .unwrap();
    }

    #[test]
    fn rejects_invalid_tracing_directives() {
        for request in [
            TracingDirectivesRequest {
                directives: "info".to_string(),
                ttl_secs: Some(MAX_TRACING_TTL_SECS + 1),
            },
            TracingDirectivesRequest { directives: "  ".to_string(), ttl_secs: Some(30) },
            TracingDirectivesRequest {
                directives: "x".repeat(MAX_TRACING_DIRECTIVES_LEN + 1),
                ttl_secs: None,
            },
        ] {
            assert_eq!(
                validate_tracing_directives(&request).unwrap_err().code(),
                jsonrpsee_types::error::INVALID_PARAMS_CODE
            );
        }
    }
}
