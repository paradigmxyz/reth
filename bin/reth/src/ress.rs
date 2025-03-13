use reth_ethereum_primitives::EthPrimitives;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::{protocol::IntoRlpxSubProtocol, NetworkProtocols};
use reth_network_api::FullNetwork;
use reth_node_api::BeaconConsensusEngineEvent;
use reth_node_core::args::RessArgs;
use reth_provider::providers::{BlockchainProvider, ProviderNodeTypes};
use reth_ress_protocol::{NodeType, ProtocolState, RessProtocolHandler};
use reth_ress_provider::{maintain_pending_state, PendingState, RethRessProtocolProvider};
use reth_tasks::TaskExecutor;
use reth_tokio_util::EventStream;
use tokio::sync::mpsc;
use tracing::*;

/// Install `ress` subprotocol if it's enabled.
pub fn install_ress_subprotocol<P, E, N>(
    args: RessArgs,
    provider: BlockchainProvider<P>,
    block_executor: E,
    network: N,
    task_executor: TaskExecutor,
    engine_events: EventStream<BeaconConsensusEngineEvent<EthPrimitives>>,
) -> eyre::Result<()>
where
    P: ProviderNodeTypes<Primitives = EthPrimitives>,
    E: BlockExecutorProvider<Primitives = EthPrimitives> + Clone,
    N: FullNetwork + NetworkProtocols,
{
    info!(target: "reth::cli", "Installing ress subprotocol");
    let pending_state = PendingState::default();

    // Spawn maintenance task for pending state.
    task_executor.spawn(maintain_pending_state(
        engine_events,
        provider.clone(),
        pending_state.clone(),
    ));

    let (tx, mut rx) = mpsc::unbounded_channel();
    let provider = RethRessProtocolProvider::new(
        provider,
        block_executor,
        Box::new(task_executor.clone()),
        args.max_witness_window,
        args.witness_max_parallel,
        args.witness_cache_size,
        pending_state,
    )?;
    network.add_rlpx_sub_protocol(
        RessProtocolHandler {
            provider,
            node_type: NodeType::Stateful,
            peers_handle: network.peers_handle().clone(),
            max_active_connections: args.max_active_connections,
            state: ProtocolState::new(tx),
        }
        .into_rlpx_sub_protocol(),
    );
    info!(target: "reth::cli", "Ress subprotocol support enabled");

    task_executor.spawn(async move {
        while let Some(event) = rx.recv().await {
            trace!(target: "reth::ress", ?event, "Received ress event");
        }
    });
    Ok(())
}
