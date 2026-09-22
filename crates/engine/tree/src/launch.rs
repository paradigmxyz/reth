//! Engine orchestrator launch helper.
//!
//! Provides [`EngineOrchestratorBuilder`](crate::launch::EngineOrchestratorBuilder) which wires
//! together all engine components and builds a
//! [`ChainOrchestrator`](crate::chain::ChainOrchestrator) ready to be polled as a `Stream`.

use crate::{
    backfill::PipelineSync,
    chain::ChainOrchestrator,
    download::BasicBlockDownloader,
    engine::{EngineApiKind, EngineApiRequest, EngineApiRequestHandler, EngineHandler},
    persistence::PersistenceHandle,
    tree::{EngineApiTreeHandler, EngineValidator, TreeConfig, WaitForCaches},
};
use futures::Stream;
use reth_consensus::FullConsensus;
use reth_engine_primitives::BeaconEngineMessage;
use reth_evm::ConfigureEvm;
use reth_network_p2p::{BlockAccessListsClient, BlockClient};
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    providers::{BlockchainProvider, ProviderNodeTypes},
    ProviderFactory,
};
use reth_prune::PrunerWithFactory;
use reth_stages_api::{MetricEventsSender, Pipeline};
use reth_storage_overlay::OverlayManager;
use reth_tasks::Runtime;
use std::sync::Arc;

/// The [`ChainOrchestrator`] built by [`EngineOrchestratorBuilder`].
pub type EngineOrchestrator<T, N, Client, S, B> = ChainOrchestrator<
    EngineHandler<
        EngineApiRequestHandler<EngineApiRequest<T, N>, N>,
        S,
        BasicBlockDownloader<Client, <N as NodePrimitives>::Block>,
    >,
    B,
>;

/// Components needed to build the engine [`ChainOrchestrator`] that drives the chain forward.
///
/// [`build`](Self::build) spawns and wires together the following components:
///
/// - **[`BasicBlockDownloader`]** — downloads blocks on demand from the network during live sync.
/// - **[`PersistenceHandle`]** — spawns the persistence service on a background thread for writing
///   blocks and performing pruning outside the critical consensus path.
/// - **[`EngineApiTreeHandler`]** — spawns the tree handler that processes engine API requests
///   (`newPayload`, `forkchoiceUpdated`) and maintains the in-memory chain state.
/// - **[`EngineApiRequestHandler`]** + **[`EngineHandler`]** — glue that routes incoming CL
///   messages to the tree handler and manages download requests.
/// - **[`PipelineSync`]** — wraps the staged sync [`Pipeline`] for backfill sync when the node
///   needs to catch up over large block ranges.
///
/// The returned orchestrator implements [`Stream`] and yields
/// [`ChainEvent`]s.
///
/// [`ChainEvent`]: crate::chain::ChainEvent
#[derive(Debug)]
pub struct EngineOrchestratorBuilder<N, Client, S, V, C>
where
    N: ProviderNodeTypes,
{
    /// The engine API flavor to run.
    pub engine_kind: EngineApiKind,
    /// Consensus used to validate downloaded and incoming blocks.
    pub consensus: Arc<dyn FullConsensus<N::Primitives>>,
    /// Network client used to download blocks during live sync.
    pub client: Client,
    /// Incoming consensus layer messages.
    pub incoming_requests: S,
    /// Staged sync pipeline used for backfill.
    pub pipeline: Pipeline<N>,
    /// Runtime the pipeline runs on.
    pub pipeline_task_spawner: Runtime,
    /// Provider factory handed to the persistence service.
    pub provider: ProviderFactory<N>,
    /// Blockchain provider backing the engine tree.
    pub blockchain_db: BlockchainProvider<N>,
    /// Pruner run by the persistence service.
    pub pruner: PrunerWithFactory<ProviderFactory<N>>,
    /// Handle to the payload builder service.
    pub payload_builder: PayloadBuilderHandle<N::Payload>,
    /// Validator for incoming payloads.
    pub payload_validator: V,
    /// Overlay manager for state on top of the database.
    pub overlay_manager: OverlayManager<N::Primitives>,
    /// Engine tree configuration.
    pub tree_config: TreeConfig,
    /// Sender for sync metric events.
    pub sync_metrics_tx: MetricEventsSender,
    /// EVM configuration used to execute payloads.
    pub evm_config: C,
    /// Runtime used to spawn engine tree tasks.
    pub runtime: Runtime,
}

impl<N, Client, S, V, C> EngineOrchestratorBuilder<N, Client, S, V, C>
where
    N: ProviderNodeTypes,
    Client: BlockClient<Block = <N::Primitives as NodePrimitives>::Block>
        + BlockAccessListsClient
        + 'static,
    S: Stream<Item = BeaconEngineMessage<N::Payload>> + Send + Sync + Unpin + 'static,
    V: EngineValidator<N::Payload> + WaitForCaches,
    C: ConfigureEvm<Primitives = N::Primitives> + 'static,
{
    /// Spawns the engine services and returns the [`ChainOrchestrator`] driving them.
    pub fn build(
        self,
    ) -> EngineOrchestrator<N::Payload, N::Primitives, Client, S, PipelineSync<N>> {
        let Self {
            engine_kind,
            consensus,
            client,
            incoming_requests,
            pipeline,
            pipeline_task_spawner,
            provider,
            blockchain_db,
            pruner,
            payload_builder,
            payload_validator,
            overlay_manager,
            tree_config,
            sync_metrics_tx,
            evm_config,
            runtime,
        } = self;

        let downloader = BasicBlockDownloader::new(client, consensus.clone());

        let persistence_handle =
            PersistenceHandle::<N::Primitives>::spawn_service(provider, pruner, sync_metrics_tx);

        let canonical_in_memory_state = blockchain_db.canonical_in_memory_state();

        let (to_tree_tx, from_tree) = EngineApiTreeHandler::spawn_new(
            blockchain_db,
            consensus,
            payload_validator,
            persistence_handle,
            payload_builder,
            canonical_in_memory_state,
            overlay_manager,
            tree_config,
            engine_kind,
            evm_config,
            runtime,
        );

        let engine_handler = EngineApiRequestHandler::new(to_tree_tx, from_tree);
        let handler = EngineHandler::new(engine_handler, downloader, incoming_requests);

        let backfill_sync = PipelineSync::new(pipeline, pipeline_task_spawner);

        ChainOrchestrator::new(handler, backfill_sync)
    }
}
