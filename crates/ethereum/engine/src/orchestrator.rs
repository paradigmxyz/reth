use futures::StreamExt;
use pin_project::pin_project;
use reth_beacon_consensus::{BeaconEngineMessage, EthBeaconConsensus};
use reth_chainspec::ChainSpec;
use reth_db_api::database::Database;
use reth_engine_primitives::EngineTypes;
use reth_engine_tree::{
    backfill::PipelineSync,
    chain::ChainOrchestrator,
    download::BasicBlockDownloader,
    engine::{EngineApiEvent, EngineApiRequestHandler, EngineHandler, FromEngine},
};
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_network_p2p::{bodies::client::BodiesClient, headers::client::HeadersClient};
use reth_stages_api::Pipeline;
use reth_tasks::TaskSpawner;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Alias for Ethereum chain orchestrator.
type EthChainOrchestratorType<DB, Client> = ChainOrchestrator<
    EngineHandler<
        EngineApiRequestHandler<EthEngineTypes>,
        UnboundedReceiverStream<BeaconEngineMessage<EthEngineTypes>>,
        BasicBlockDownloader<Client>,
    >,
    PipelineSync<DB>,
>;

/// Builder for `EthChainOrchestrator`.
#[allow(missing_debug_implementations, dead_code)]
pub struct EthChainOrchestratorBuilder<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    chain_spec: Option<Arc<ChainSpec>>,
    client: Option<Client>,
    incoming_requests: Option<UnboundedReceiverStream<BeaconEngineMessage<EthEngineTypes>>>,
    tree_channels: Option<TreeChannels<EthEngineTypes>>,
    pipeline_container: Option<PipelineContainer<DB>>,
}

impl<DB, Client> Default for EthChainOrchestratorBuilder<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    fn default() -> Self {
        Self {
            chain_spec: None,
            client: None,
            incoming_requests: None,
            tree_channels: None,
            pipeline_container: None,
        }
    }
}

#[allow(dead_code)]
impl<DB, Client> EthChainOrchestratorBuilder<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    fn with_chain_spec(mut self, chain_spec: Arc<ChainSpec>) -> Self {
        self.chain_spec = Some(chain_spec);
        self
    }

    fn with_client(mut self, client: Client) -> Self {
        self.client = Some(client);
        self
    }

    fn with_incoming_requests(
        mut self,
        incoming_requests: UnboundedReceiverStream<BeaconEngineMessage<EthEngineTypes>>,
    ) -> Self {
        self.incoming_requests = Some(incoming_requests);
        self
    }

    fn with_tree_channels(
        mut self,
        to_tree: mpsc::Sender<FromEngine<BeaconEngineMessage<EthEngineTypes>>>,
        from_tree: mpsc::UnboundedReceiver<EngineApiEvent>,
    ) -> Self {
        self.tree_channels = Some(TreeChannels { to_tree, from_tree });
        self
    }

    fn with_pipeline_and_spawner(
        mut self,
        pipeline: Pipeline<DB>,
        pipeline_task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        self.pipeline_container = Some(PipelineContainer { pipeline, pipeline_task_spawner });
        self
    }

    fn build(self) -> eyre::Result<EthChainOrchestrator<DB, Client>> {
        let (
            Some(chain_spec),
            Some(client),
            Some(incoming_requests),
            Some(tree_channels),
            Some(pipeline_container),
        ) = (
            self.chain_spec,
            self.client,
            self.incoming_requests,
            self.tree_channels,
            self.pipeline_container,
        )
        else {
            eyre::bail!("not all required compoenents specified")
        };

        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec));
        let downloader = BasicBlockDownloader::new(client, consensus);

        let TreeChannels { to_tree, from_tree } = tree_channels;
        let engine_handler = EngineApiRequestHandler::new(to_tree, from_tree);
        let handler = EngineHandler::new(engine_handler, downloader, incoming_requests);

        let PipelineContainer { pipeline, pipeline_task_spawner } = pipeline_container;
        let backfill_sync = PipelineSync::new(pipeline, pipeline_task_spawner);

        Ok(EthChainOrchestrator { orchestrator: ChainOrchestrator::new(handler, backfill_sync) })
    }
}

/// The type that drives the Ethereum chain forward and communicates progress.
#[pin_project]
#[allow(missing_debug_implementations)]
pub struct EthChainOrchestrator<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    orchestrator: EthChainOrchestratorType<DB, Client>,
}

impl<DB, Client> Future for EthChainOrchestrator<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    type Output = Result<(), EthChainOrchestratorError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Call poll on the inner orchestrator.
        match self.project().orchestrator.poll_next_unpin(cx) {
            Poll::Ready(Some(_event)) => Poll::Ready(Ok(())),
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Potential error returned by `EthChainOrchestrator`.
#[derive(Debug)]
pub struct EthChainOrchestratorError {}

/// Helper container for tree channels in `EthChainOrchestratorBuilder`.
struct TreeChannels<T: EngineTypes> {
    to_tree: mpsc::Sender<FromEngine<BeaconEngineMessage<T>>>,
    from_tree: mpsc::UnboundedReceiver<EngineApiEvent>,
}

/// Helper container for pipeline related values in `EthChainOrchestratorBuilder`.
struct PipelineContainer<DB: Database> {
    pipeline: Pipeline<DB>,
    pipeline_task_spawner: Box<dyn TaskSpawner>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_engine_tree::test_utils::TestPipelineBuilder;
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_network_p2p::test_utils::TestFullBlockClient;
    use reth_tasks::TokioTaskExecutor;
    use std::sync::Arc;

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

        let (_tx, rx) = mpsc::unbounded_channel::<BeaconEngineMessage<EthEngineTypes>>();
        let incoming_requests = UnboundedReceiverStream::new(rx);

        let pipeline = TestPipelineBuilder::new().build(chain_spec.clone());
        let pipeline_task_spawner = Box::<TokioTaskExecutor>::default();

        let (to_tree_tx, _to_tree_rx) = mpsc::channel(32);
        let (_from_tree_tx, from_tree_rx) = mpsc::unbounded_channel();

        let _eth_chain_orchestrator = EthChainOrchestratorBuilder::default()
            .with_chain_spec(chain_spec)
            .with_client(client)
            .with_incoming_requests(incoming_requests)
            .with_tree_channels(to_tree_tx, from_tree_rx)
            .with_pipeline_and_spawner(pipeline, pipeline_task_spawner)
            .build()
            .unwrap();
    }
}
