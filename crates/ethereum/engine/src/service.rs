use futures::{ready, StreamExt};
use pin_project::pin_project;
use reth_beacon_consensus::{BeaconEngineMessage, EthBeaconConsensus};
use reth_chainspec::ChainSpec;
use reth_db_api::database::Database;
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
    sync::{mpsc::Sender, Arc},
    task::{Context, Poll},
};
use tokio::sync::mpsc::UnboundedReceiver;
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
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        client: Client,
        to_tree: Sender<FromEngine<BeaconEngineMessage<EthEngineTypes>>>,
        from_tree: UnboundedReceiver<EngineApiEvent>,
        incoming_requests: UnboundedReceiverStream<BeaconEngineMessage<EthEngineTypes>>,
        pipeline: Pipeline<DB>,
        pipeline_task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec));
        let downloader = BasicBlockDownloader::new(client, consensus);

        let engine_handler = EngineApiRequestHandler::new(to_tree, from_tree);
        let handler = EngineHandler::new(engine_handler, downloader, incoming_requests);

        let backfill_sync = PipelineSync::new(pipeline, pipeline_task_spawner);

        Self { orchestrator: ChainOrchestrator::new(handler, backfill_sync) }
    }
}

impl<DB, Client> Future for EthService<DB, Client>
where
    DB: Database + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    type Output = Result<(), EthServiceError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Call poll on the inner orchestrator.
        let mut orchestrator = self.project().orchestrator;
        loop {
            match ready!(StreamExt::poll_next_unpin(&mut orchestrator, cx)) {
                Some(_event) => continue,
                None => return Poll::Ready(Ok(())),
            }
        }
    }
}

/// Potential error returned by `EthService`.
#[derive(Debug, thiserror::Error)]
#[error("Eth service error.")]
pub struct EthServiceError {}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_engine_tree::test_utils::TestPipelineBuilder;
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_network_p2p::test_utils::TestFullBlockClient;
    use reth_tasks::TokioTaskExecutor;
    use std::sync::{mpsc::channel, Arc};
    use tokio::sync::mpsc::unbounded_channel;

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

        let (to_tree_tx, _to_tree_rx) = channel();
        let (_from_tree_tx, from_tree_rx) = unbounded_channel();

        let _eth_chain_orchestrator = EthService::new(
            chain_spec,
            client,
            to_tree_tx,
            from_tree_rx,
            incoming_requests,
            pipeline,
            pipeline_task_spawner,
        );
    }
}
