use futures::{Stream, StreamExt};
use pin_project::pin_project;
use reth_beacon_consensus::{BeaconConsensusEngineEvent, BeaconEngineMessage, EthBeaconConsensus};
use reth_chainspec::ChainSpec;
use reth_db_api::database::Database;
use reth_engine_tree::{
    backfill::PipelineSync,
    download::BasicBlockDownloader,
    engine::{EngineApiRequestHandler, EngineHandler},
    persistence::PersistenceHandle,
    tree::EngineApiTreeHandlerImpl,
};
pub use reth_engine_tree::{
    chain::{ChainEvent, ChainOrchestrator},
    engine::EngineApiEvent,
};
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_evm_ethereum::execute::EthExecutorProvider;
use reth_network_p2p::{bodies::client::BodiesClient, headers::client::HeadersClient};
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_validator::ExecutionPayloadValidator;
use reth_provider::{providers::BlockchainProvider, ProviderFactory};
use reth_prune::Pruner;
use reth_stages_api::Pipeline;
use reth_tasks::TaskSpawner;
use std::{
    pin::Pin,
    sync::{mpsc::channel, Arc},
    task::{Context, Poll},
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Alias for Ethereum chain orchestrator.
type EthServiceType<DB, Client> = ChainOrchestrator<
    EngineHandler<
        EngineApiRequestHandler<EthEngineTypes>,
        UnboundedReceiverStream<BeaconEngineMessage<EthEngineTypes>>,
        BasicBlockDownloader<Client>,
    >,
    PipelineSync<DB>,
>;

/// The type that drives the Ethereum chain forward and communicates progress.
#[pin_project]
#[allow(missing_debug_implementations)]
pub struct EthService<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    orchestrator: EthServiceType<DB, Client>,
}

impl<DB, Client> EthService<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    /// Constructor for `EthService`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        client: Client,
        incoming_requests: UnboundedReceiverStream<BeaconEngineMessage<EthEngineTypes>>,
        pipeline: Pipeline<DB>,
        pipeline_task_spawner: Box<dyn TaskSpawner>,
        provider: ProviderFactory<DB>,
        blockchain_db: BlockchainProvider<DB>,
        pruner: Pruner<DB, ProviderFactory<DB>>,
        payload_builder: PayloadBuilderHandle<EthEngineTypes>,
    ) -> Self {
        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));
        let downloader = BasicBlockDownloader::new(client, consensus.clone());

        let (to_tree_tx, to_tree_rx) = channel();

        let persistence_handle = PersistenceHandle::spawn_services(provider, pruner);
        let payload_validator = ExecutionPayloadValidator::new(chain_spec.clone());
        let executor_factory = EthExecutorProvider::ethereum(chain_spec);

        let from_tree = EngineApiTreeHandlerImpl::spawn_new(
            blockchain_db,
            executor_factory,
            consensus,
            payload_validator,
            to_tree_rx,
            persistence_handle,
            payload_builder,
        );

        let engine_handler = EngineApiRequestHandler::new(to_tree_tx, from_tree);
        let handler = EngineHandler::new(engine_handler, downloader, incoming_requests);

        let backfill_sync = PipelineSync::new(pipeline, pipeline_task_spawner);

        Self { orchestrator: ChainOrchestrator::new(handler, backfill_sync) }
    }

    /// Returns a mutable reference to the orchestrator.
    pub fn orchestrator_mut(&mut self) -> &mut EthServiceType<DB, Client> {
        &mut self.orchestrator
    }
}

impl<DB, Client> Stream for EthService<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    type Item = ChainEvent<BeaconConsensusEngineEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut orchestrator = self.project().orchestrator;
        StreamExt::poll_next_unpin(&mut orchestrator, cx)
    }
}

/// Potential error returned by `EthService`.
#[derive(Debug, thiserror::Error)]
#[error("Eth service error.")]
pub struct EthServiceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_blockchain_tree::{
        BlockchainTree, BlockchainTreeConfig, ShareableBlockchainTree, TreeExternals,
    };
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_consensus::test_utils::TestConsensus;
    use reth_engine_tree::test_utils::TestPipelineBuilder;
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_exex_types::FinishedExExHeight;
    use reth_network_p2p::test_utils::TestFullBlockClient;
    use reth_primitives::SealedHeader;
    use reth_provider::test_utils::create_test_provider_factory_with_chain_spec;
    use reth_prune::PruneModes;
    use reth_tasks::TokioTaskExecutor;
    use std::sync::Arc;
    use tokio::sync::{mpsc::unbounded_channel, watch};

    #[test]
    fn eth_chain_orchestrator_build() {
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );

        let client = TestFullBlockClient::default();

        let (_tx, rx) = unbounded_channel::<BeaconEngineMessage<EthEngineTypes>>();
        let incoming_requests = UnboundedReceiverStream::new(rx);

        let pipeline = TestPipelineBuilder::new().build(chain_spec.clone());
        let pipeline_task_spawner = Box::<TokioTaskExecutor>::default();
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        let consensus = Arc::new(TestConsensus::default());
        let executor_factory = EthExecutorProvider::ethereum(chain_spec.clone());
        let externals = TreeExternals::new(provider_factory.clone(), consensus, executor_factory);
        let tree = Arc::new(ShareableBlockchainTree::new(
            BlockchainTree::new(
                externals,
                BlockchainTreeConfig::new(1, 2, 3, 2),
                PruneModes::default(),
            )
            .expect("failed to create tree"),
        ));

        let blockchain_db = BlockchainProvider::with_latest(
            provider_factory.clone(),
            tree,
            SealedHeader::default(),
        );

        let (_tx, rx) = watch::channel(FinishedExExHeight::NoExExs);
        let pruner =
            Pruner::<_, ProviderFactory<_>>::new(provider_factory.clone(), vec![], 0, 0, None, rx);

        let (tx, _rx) = unbounded_channel();
        let _eth_service = EthService::new(
            chain_spec,
            client,
            incoming_requests,
            pipeline,
            pipeline_task_spawner,
            provider_factory,
            blockchain_db,
            pruner,
            PayloadBuilderHandle::new(tx),
        );
    }
}
