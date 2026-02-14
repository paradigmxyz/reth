//! Engine orchestrator launch helper.

use crate::{
    backfill::PipelineSync,
    chain::ChainOrchestrator,
    download::BasicBlockDownloader,
    engine::{EngineApiKind, EngineApiRequest, EngineApiRequestHandler, EngineHandler},
    persistence::PersistenceHandle,
    tree::{EngineApiTreeHandler, EngineValidator, TreeConfig},
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
    ProviderFactory, StorageSettingsCache,
};
use reth_prune::PrunerWithFactory;
use reth_stages_api::{MetricEventsSender, Pipeline};
use reth_tasks::TaskSpawner;
use reth_trie_db::ChangesetCache;
use std::sync::Arc;

/// Builds the engine orchestrator that drives the chain forward.
///
/// This wires together the block downloader, persistence layer, engine API tree handler,
/// and pipeline sync into a [`ChainOrchestrator`].
#[expect(clippy::too_many_arguments)]
pub fn build_engine_orchestrator<N, Client, S, V, C>(
    engine_kind: EngineApiKind,
    consensus: Arc<dyn FullConsensus<N::Primitives>>,
    client: Client,
    incoming_requests: S,
    pipeline: Pipeline<N>,
    pipeline_task_spawner: Box<dyn TaskSpawner>,
    provider: ProviderFactory<N>,
    blockchain_db: BlockchainProvider<N>,
    pruner: PrunerWithFactory<ProviderFactory<N>>,
    payload_builder: PayloadBuilderHandle<N::Payload>,
    payload_validator: V,
    tree_config: TreeConfig,
    sync_metrics_tx: MetricEventsSender,
    evm_config: C,
    changeset_cache: ChangesetCache,
) -> ChainOrchestrator<
    EngineHandler<
        EngineApiRequestHandler<EngineApiRequest<N::Payload, N::Primitives>, N::Primitives>,
        S,
        BasicBlockDownloader<Client, <N::Primitives as NodePrimitives>::Block>,
    >,
    PipelineSync<N>,
>
where
    N: ProviderNodeTypes,
    Client: BlockClient<Block = <N::Primitives as NodePrimitives>::Block> + 'static,
    S: Stream<Item = BeaconEngineMessage<N::Payload>> + Send + Sync + Unpin + 'static,
    V: EngineValidator<N::Payload>,
    C: ConfigureEvm<Primitives = N::Primitives> + 'static,
{
    let downloader = BasicBlockDownloader::new(client, consensus.clone());
    let use_hashed_state = provider.cached_storage_settings().use_hashed_state();

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
        tree_config,
        engine_kind,
        evm_config,
        changeset_cache,
        use_hashed_state,
    );

    let engine_handler = EngineApiRequestHandler::new(to_tree_tx, from_tree);
    let handler = EngineHandler::new(engine_handler, downloader, incoming_requests);

    let backfill_sync = PipelineSync::new(pipeline, pipeline_task_spawner);

    ChainOrchestrator::new(handler, backfill_sync)
}
