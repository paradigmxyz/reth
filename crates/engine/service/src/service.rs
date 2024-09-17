use futures::{Stream, StreamExt};
use pin_project::pin_project;
use reth_beacon_consensus::{BeaconConsensusEngineEvent, BeaconEngineMessage, EngineNodeTypes};
use reth_consensus::Consensus;
use reth_engine_tree::{
    backfill::PipelineSync,
    download::BasicBlockDownloader,
    engine::{EngineApiRequest, EngineApiRequestHandler, EngineHandler},
    persistence::PersistenceHandle,
    tree::{EngineApiTreeHandler, InvalidBlockHook, TreeConfig},
};
pub use reth_engine_tree::{
    chain::{ChainEvent, ChainOrchestrator},
    engine::EngineApiEvent,
};
use reth_evm::execute::BlockExecutorProvider;
use reth_network_p2p::BlockClient;
use reth_node_types::NodeTypesWithEngine;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_validator::ExecutionPayloadValidator;
use reth_provider::{providers::BlockchainProvider2, ProviderFactory};
use reth_prune::PrunerWithFactory;
use reth_stages_api::{MetricEventsSender, Pipeline};
use reth_tasks::TaskSpawner;
use std::{
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// Alias for consensus engine stream.
type EngineMessageStream<T> = Pin<Box<dyn Stream<Item = BeaconEngineMessage<T>> + Send + Sync>>;

/// Alias for chain orchestrator.
type EngineServiceType<N, Client> = ChainOrchestrator<
    EngineHandler<
        EngineApiRequestHandler<EngineApiRequest<<N as NodeTypesWithEngine>::Engine>>,
        EngineMessageStream<<N as NodeTypesWithEngine>::Engine>,
        BasicBlockDownloader<Client>,
    >,
    PipelineSync<N>,
>;

/// The type that drives the chain forward and communicates progress.
#[pin_project]
#[allow(missing_debug_implementations)]
pub struct EngineService<N, Client, E>
where
    N: EngineNodeTypes,
    Client: BlockClient + 'static,
    E: BlockExecutorProvider + 'static,
{
    orchestrator: EngineServiceType<N, Client>,
    _marker: PhantomData<E>,
}

impl<N, Client, E> EngineService<N, Client, E>
where
    N: EngineNodeTypes,
    Client: BlockClient + 'static,
    E: BlockExecutorProvider + 'static,
{
    /// Constructor for `EngineService`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        consensus: Arc<dyn Consensus>,
        executor_factory: E,
        chain_spec: Arc<N::ChainSpec>,
        client: Client,
        incoming_requests: EngineMessageStream<N::Engine>,
        pipeline: Pipeline<N>,
        pipeline_task_spawner: Box<dyn TaskSpawner>,
        provider: ProviderFactory<N>,
        blockchain_db: BlockchainProvider2<N>,
        pruner: PrunerWithFactory<ProviderFactory<N>>,
        payload_builder: PayloadBuilderHandle<N::Engine>,
        tree_config: TreeConfig,
        invalid_block_hook: Box<dyn InvalidBlockHook>,
        sync_metrics_tx: MetricEventsSender,
    ) -> Self {
        let downloader = BasicBlockDownloader::new(client, consensus.clone());

        let persistence_handle =
            PersistenceHandle::spawn_service(provider, pruner, sync_metrics_tx);
        let payload_validator = ExecutionPayloadValidator::new(chain_spec);

        let canonical_in_memory_state = blockchain_db.canonical_in_memory_state();

        let (to_tree_tx, from_tree) = EngineApiTreeHandler::spawn_new(
            blockchain_db,
            executor_factory,
            consensus,
            payload_validator,
            persistence_handle,
            payload_builder,
            canonical_in_memory_state,
            tree_config,
            invalid_block_hook,
        );

        let engine_handler = EngineApiRequestHandler::new(to_tree_tx, from_tree);
        let handler = EngineHandler::new(engine_handler, downloader, incoming_requests);

        let backfill_sync = PipelineSync::new(pipeline, pipeline_task_spawner);

        Self {
            orchestrator: ChainOrchestrator::new(handler, backfill_sync),
            _marker: Default::default(),
        }
    }

    /// Returns a mutable reference to the orchestrator.
    pub fn orchestrator_mut(&mut self) -> &mut EngineServiceType<N, Client> {
        &mut self.orchestrator
    }
}

impl<N, Client, E> Stream for EngineService<N, Client, E>
where
    N: EngineNodeTypes,
    Client: BlockClient + 'static,
    E: BlockExecutorProvider + 'static,
{
    type Item = ChainEvent<BeaconConsensusEngineEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut orchestrator = self.project().orchestrator;
        StreamExt::poll_next_unpin(&mut orchestrator, cx)
    }
}

/// Potential error returned by `EngineService`.
#[derive(Debug, thiserror::Error)]
#[error("Engine service error.")]
pub struct EngineServiceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_beacon_consensus::EthBeaconConsensus;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_engine_tree::{test_utils::TestPipelineBuilder, tree::NoopInvalidBlockHook};
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_exex_types::FinishedExExHeight;
    use reth_network_p2p::test_utils::TestFullBlockClient;
    use reth_primitives::SealedHeader;
    use reth_provider::test_utils::create_test_provider_factory_with_chain_spec;
    use reth_prune::Pruner;
    use reth_tasks::TokioTaskExecutor;
    use std::sync::Arc;
    use tokio::sync::{mpsc::unbounded_channel, watch};
    use tokio_stream::wrappers::UnboundedReceiverStream;

    #[test]
    fn eth_chain_orchestrator_build() {
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );
        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));

        let client = TestFullBlockClient::default();

        let (_tx, rx) = unbounded_channel::<BeaconEngineMessage<EthEngineTypes>>();
        let incoming_requests = UnboundedReceiverStream::new(rx);

        let pipeline = TestPipelineBuilder::new().build(chain_spec.clone());
        let pipeline_task_spawner = Box::<TokioTaskExecutor>::default();
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());

        let executor_factory = EthExecutorProvider::ethereum(chain_spec.clone());
        let blockchain_db =
            BlockchainProvider2::with_latest(provider_factory.clone(), SealedHeader::default())
                .unwrap();

        let (_tx, rx) = watch::channel(FinishedExExHeight::NoExExs);
        let pruner = Pruner::new_with_factory(provider_factory.clone(), vec![], 0, 0, None, rx);

        let (sync_metrics_tx, _sync_metrics_rx) = unbounded_channel();
        let (tx, _rx) = unbounded_channel();
        let _eth_service = EngineService::new(
            consensus,
            executor_factory,
            chain_spec,
            client,
            Box::pin(incoming_requests),
            pipeline,
            pipeline_task_spawner,
            provider_factory,
            blockchain_db,
            pruner,
            PayloadBuilderHandle::new(tx),
            TreeConfig::default(),
            Box::new(NoopInvalidBlockHook::default()),
            sync_metrics_tx,
        );
    }
}
