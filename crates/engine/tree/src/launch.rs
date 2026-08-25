//! Engine orchestrator launch helpers.
//!
//! Both wire the engine components into a [`ChainOrchestrator`](crate::chain::ChainOrchestrator)
//! ready to be polled as a `Stream`. They differ only in how the node backfills:
//! [`build_engine_orchestrator`](crate::launch::build_engine_orchestrator) uses the staged
//! [`Pipeline`](reth_stages_api::Pipeline), while
//! [`build_engine_orchestrator_with_backfill`](crate::launch::build_engine_orchestrator_with_backfill)
//! takes any [`BackfillSync`](crate::backfill::BackfillSync).

use crate::{
    backfill::{BackfillSync, PipelineSync},
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
use reth_network_p2p::BlockClient;
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

/// The [`EngineHandler`] the orchestrator drives.
pub type EngineChainHandler<Payload, Primitives, S, Client> = EngineHandler<
    EngineApiRequestHandler<EngineApiRequest<Payload, Primitives>, Primitives>,
    S,
    BasicBlockDownloader<Client, <Primitives as NodePrimitives>::Block>,
>;

/// The [`ChainOrchestrator`] returned by the launch helpers, generic over its backfill mechanism.
pub type EngineOrchestrator<Payload, Primitives, S, Client, B> =
    ChainOrchestrator<EngineChainHandler<Payload, Primitives, S, Client>, B>;

/// The components an engine [`ChainOrchestrator`] is assembled from.
#[derive(Debug)]
pub struct EngineOrchestratorConfig<N: ProviderNodeTypes, Client, S, V, C> {
    /// Selects Ethereum or OP Stack engine API semantics.
    pub engine_kind: EngineApiKind,
    /// Validates blocks against consensus rules.
    pub consensus: Arc<dyn FullConsensus<N::Primitives>>,
    /// Downloads blocks on demand during live sync.
    pub client: Client,
    /// Stream of messages from the consensus layer.
    pub incoming_requests: S,
    /// Database handle the persistence service writes through.
    pub provider: ProviderFactory<N>,
    /// Provider the tree handler reads canonical and in-memory state from.
    pub blockchain_db: BlockchainProvider<N>,
    /// Prunes historical data outside the consensus path.
    pub pruner: PrunerWithFactory<ProviderFactory<N>>,
    /// Handle used to request payload builds.
    pub payload_builder: PayloadBuilderHandle<N::Payload>,
    /// Validates engine API payloads.
    pub payload_validator: V,
    /// Tracks state overlays for in-memory blocks.
    pub overlay_manager: OverlayManager<N::Primitives>,
    /// Tuning for the engine tree handler.
    pub tree_config: TreeConfig,
    /// Sink for sync metrics.
    pub sync_metrics_tx: MetricEventsSender,
    /// EVM configuration used to execute payloads.
    pub evm_config: C,
    /// Spawns the engine's background tasks.
    pub runtime: Runtime,
}

/// Builds the engine [`ChainOrchestrator`], backfilling with the staged [`Pipeline`].
///
/// Spawns and wires together:
///
/// - **[`BasicBlockDownloader`]** — downloads blocks on demand during live sync.
/// - **[`PersistenceHandle`]** — writes blocks and prunes off the consensus path.
/// - **[`EngineApiTreeHandler`]** — serves engine API requests and owns in-memory chain state.
/// - **[`EngineApiRequestHandler`]** + **[`EngineHandler`]** — route CL messages to the tree.
/// - **[`PipelineSync`]** — backfills over large block ranges.
///
/// The result yields [`ChainEvent`]s as a [`Stream`].
///
/// [`ChainEvent`]: crate::chain::ChainEvent
pub fn build_engine_orchestrator<N, Client, S, V, C>(
    config: EngineOrchestratorConfig<N, Client, S, V, C>,
    pipeline: Pipeline<N>,
    pipeline_task_spawner: Runtime,
) -> EngineOrchestrator<N::Payload, N::Primitives, S, Client, PipelineSync<N>>
where
    N: ProviderNodeTypes,
    Client: BlockClient<Block = <N::Primitives as NodePrimitives>::Block> + 'static,
    S: Stream<Item = BeaconEngineMessage<N::Payload>> + Send + Sync + Unpin + 'static,
    V: EngineValidator<N::Payload> + WaitForCaches,
    C: ConfigureEvm<Primitives = N::Primitives> + 'static,
{
    build_engine_orchestrator_with_backfill(
        config,
        PipelineSync::new(pipeline, pipeline_task_spawner),
    )
}

/// Builds the engine [`ChainOrchestrator`] on a caller-supplied [`BackfillSync`].
///
/// Wires the same components as [`build_engine_orchestrator`], letting the node substitute a
/// backfill mechanism — a snapshot bootstrap, say — that this crate does not depend on.
pub fn build_engine_orchestrator_with_backfill<N, Client, S, V, C, B>(
    config: EngineOrchestratorConfig<N, Client, S, V, C>,
    backfill_sync: B,
) -> EngineOrchestrator<N::Payload, N::Primitives, S, Client, B>
where
    N: ProviderNodeTypes,
    Client: BlockClient<Block = <N::Primitives as NodePrimitives>::Block> + 'static,
    S: Stream<Item = BeaconEngineMessage<N::Payload>> + Send + Sync + Unpin + 'static,
    V: EngineValidator<N::Payload> + WaitForCaches,
    C: ConfigureEvm<Primitives = N::Primitives> + 'static,
    B: BackfillSync + Unpin,
{
    let EngineOrchestratorConfig {
        engine_kind,
        consensus,
        client,
        incoming_requests,
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
    } = config;

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

    ChainOrchestrator::new(handler, backfill_sync)
}
