use reth_ethereum_primitives::EthPrimitives;
use reth_evm::ConfigureEvm;
use reth_network::{protocol::IntoRlpxSubProtocol, NetworkProtocols};
use reth_network_api::FullNetwork;
use reth_provider::providers::{BlockchainProvider, ProviderNodeTypes};
use reth_ress_protocol::NodeType;
use reth_ress_provider::RethRessProtocolProvider;
use reth_tasks::TaskExecutor;
use reth_zk_ress_protocol::{ProtocolState, ZkRessProtocolHandler};
use reth_zk_ress_provider::{RethZkRessProtocolProvider, ZkRessProver};
use tokio::sync::mpsc;
use tracing::*;

/// Install `zkress` subprotocol if it's enabled.
pub fn install_zk_ress_subprotocol<P, E, N>(
    prover: ZkRessProver,
    protocol_version: usize,
    provider: RethRessProtocolProvider<BlockchainProvider<P>, E>,
    network: N,
    task_executor: TaskExecutor,
    max_active_connections: u64,
) -> eyre::Result<()>
where
    P: ProviderNodeTypes<Primitives = EthPrimitives>,
    E: ConfigureEvm<Primitives = EthPrimitives> + Clone + 'static,
    N: FullNetwork + NetworkProtocols,
{
    let protocol_name = prover.protocol_name();
    info!(target: "reth::cli", name = %protocol_name, version = protocol_version, "Installing zkress subprotocol");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let provider = RethZkRessProtocolProvider::new(provider, prover);
    network.add_rlpx_sub_protocol(
        ZkRessProtocolHandler {
            protocol_name,
            protocol_version,
            provider,
            node_type: NodeType::Stateful,
            peers_handle: network.peers_handle().clone(),
            max_active_connections,
            state: ProtocolState::new(tx),
        }
        .into_rlpx_sub_protocol(),
    );
    info!(target: "reth::cli", name = %protocol_name, version = protocol_version, "zkress subprotocol support enabled");

    task_executor.spawn(async move {
        while let Some(event) = rx.recv().await {
            trace!(target: "reth::ress", name = %protocol_name, version = protocol_version, ?event, "Received zkress event");
        }
    });

    Ok(())
}
