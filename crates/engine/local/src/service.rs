//! Provides a local dev service engine that can be used to run a dev chain.
//!
//! [`LocalEngineService`] polls the payload builder based on a mining mode
//! which can be set to `Instant` or `Interval`. The `Instant` mode will
//! constantly poll the payload builder and initiate block building
//! with a single transaction. The `Interval` mode will initiate block
//! building at a fixed interval.

use core::fmt;
use std::{
    fmt::{Debug, Formatter},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use crate::miner::{LocalMiner, MiningMode};
use futures_util::{Stream, StreamExt};
use reth_beacon_consensus::{BeaconConsensusEngineEvent, BeaconEngineMessage, EngineNodeTypes};
use reth_chainspec::EthChainSpec;
use reth_consensus::Consensus;
use reth_engine_service::service::EngineMessageStream;
use reth_engine_tree::{
    chain::{ChainEvent, HandlerEvent},
    engine::{
        EngineApiKind, EngineApiRequest, EngineApiRequestHandler, EngineRequestHandler, FromEngine,
        RequestHandlerEvent,
    },
    persistence::PersistenceHandle,
    tree::{EngineApiTreeHandler, InvalidBlockHook, TreeConfig},
};
use reth_evm::execute::BlockExecutorProvider;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{PayloadAttributesBuilder, PayloadTypes};
use reth_payload_validator::ExecutionPayloadValidator;
use reth_provider::{providers::BlockchainProvider2, ChainSpecProvider, ProviderFactory};
use reth_prune::PrunerWithFactory;
use reth_stages_api::MetricEventsSender;
use tokio::sync::mpsc::UnboundedSender;
use tracing::error;

/// Provides a local dev service engine that can be used to drive the
/// chain forward.
///
/// This service both produces and consumes [`BeaconEngineMessage`]s. This is done to allow
/// modifications of the stream
pub struct LocalEngineService<N>
where
    N: EngineNodeTypes,
{
    /// Processes requests.
    ///
    /// This type is responsible for processing incoming requests.
    handler: EngineApiRequestHandler<EngineApiRequest<N::Engine>>,
    /// Receiver for incoming requests (from the engine API endpoint) that need to be processed.
    incoming_requests: EngineMessageStream<N::Engine>,
}

impl<N> LocalEngineService<N>
where
    N: EngineNodeTypes,
{
    /// Constructor for [`LocalEngineService`].
    #[allow(clippy::too_many_arguments)]
    pub fn new<B>(
        consensus: Arc<dyn Consensus>,
        executor_factory: impl BlockExecutorProvider,
        provider: ProviderFactory<N>,
        blockchain_db: BlockchainProvider2<N>,
        pruner: PrunerWithFactory<ProviderFactory<N>>,
        payload_builder: PayloadBuilderHandle<N::Engine>,
        tree_config: TreeConfig,
        invalid_block_hook: Box<dyn InvalidBlockHook>,
        sync_metrics_tx: MetricEventsSender,
        to_engine: UnboundedSender<BeaconEngineMessage<N::Engine>>,
        from_engine: EngineMessageStream<N::Engine>,
        mode: MiningMode,
        payload_attributes_builder: B,
    ) -> Self
    where
        B: PayloadAttributesBuilder<<N::Engine as PayloadTypes>::PayloadAttributes>,
    {
        let chain_spec = provider.chain_spec();
        let engine_kind =
            if chain_spec.is_optimism() { EngineApiKind::OpStack } else { EngineApiKind::Ethereum };

        let persistence_handle =
            PersistenceHandle::spawn_service(provider.clone(), pruner, sync_metrics_tx);
        let payload_validator = ExecutionPayloadValidator::new(chain_spec);

        let canonical_in_memory_state = blockchain_db.canonical_in_memory_state();

        let (to_tree_tx, from_tree) = EngineApiTreeHandler::spawn_new(
            blockchain_db.clone(),
            executor_factory,
            consensus,
            payload_validator,
            persistence_handle,
            payload_builder.clone(),
            canonical_in_memory_state,
            tree_config,
            invalid_block_hook,
            engine_kind,
        );

        let handler = EngineApiRequestHandler::new(to_tree_tx, from_tree);

        LocalMiner::spawn_new(
            blockchain_db,
            payload_attributes_builder,
            to_engine,
            mode,
            payload_builder,
        );

        Self { handler, incoming_requests: from_engine }
    }
}

impl<N> Stream for LocalEngineService<N>
where
    N: EngineNodeTypes,
{
    type Item = ChainEvent<BeaconConsensusEngineEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        if let Poll::Ready(ev) = this.handler.poll(cx) {
            return match ev {
                RequestHandlerEvent::HandlerEvent(ev) => match ev {
                    HandlerEvent::BackfillAction(_) => {
                        error!(target: "engine::local", "received backfill request in local engine");
                        Poll::Ready(Some(ChainEvent::FatalError))
                    }
                    HandlerEvent::Event(ev) => Poll::Ready(Some(ChainEvent::Handler(ev))),
                    HandlerEvent::FatalError => Poll::Ready(Some(ChainEvent::FatalError)),
                },
                RequestHandlerEvent::Download(_) => {
                    error!(target: "engine::local", "received download request in local engine");
                    Poll::Ready(Some(ChainEvent::FatalError))
                }
            }
        }

        // forward incoming requests to the handler
        while let Poll::Ready(Some(req)) = this.incoming_requests.poll_next_unpin(cx) {
            this.handler.on_event(FromEngine::Request(req.into()));
        }

        Poll::Pending
    }
}

impl<N: EngineNodeTypes> Debug for LocalEngineService<N> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalEngineService").finish_non_exhaustive()
    }
}

// #[cfg(test)]
// mod tests {
//     use crate::LocalPayloadAttributesBuilder;

//     use super::*;
//     use reth_basic_payload_builder::BasicPayloadJobGenerator;
//     use reth_beacon_consensus::EthBeaconConsensus;
//     use reth_chain_state::CanonicalInMemoryState;
//     use reth_chainspec::{DEV, MAINNET};
//     use reth_config::PruneConfig;
//     use reth_db::test_utils::{create_test_rw_db, create_test_static_files_dir};
//     use reth_db_common::init::init_genesis;
//     use reth_engine_tree::tree::NoopInvalidBlockHook;
//     use reth_ethereum_engine_primitives::EthEngineTypes;
//     use reth_evm_ethereum::{execute::EthExecutorProvider, EthEvmConfig};
//     use reth_exex_test_utils::TestNode;
//     use reth_node_types::NodeTypesWithDBAdapter;
//     use reth_payload_builder::{test_utils::spawn_test_payload_service, PayloadBuilderService};
//     use reth_provider::{
//         providers::StaticFileProvider,
//         test_utils::{create_test_provider_factory, create_test_provider_factory_with_chain_spec},
//         BlockReader, CanonStateSubscriptions, ProviderFactory,
//     };
//     use reth_prune::PrunerBuilder;
//     use reth_transaction_pool::{
//         test_utils::{testing_pool, MockTransaction},
//         TransactionPool,
//     };
//     use std::{convert::Infallible, time::Duration};
//     use tokio::sync::mpsc::unbounded_channel;
//     use tokio_stream::wrappers::UnboundedReceiverStream;

//     #[tokio::test]
//     async fn test_local_engine_service_interval() -> eyre::Result<()> {
//         reth_tracing::init_test_tracing();

//         // Start the providers
//         let (_, static_dir_path) = create_test_static_files_dir();
//         let provider = create_test_provider_factory_with_chain_spec(DEV.clone());
//         init_genesis(&provider)?;

//         // Start a transaction pool
//         let pool = testing_pool();

//         let blockchain_db = BlockchainProvider2::new(provider.clone())?;

//         // Configure pruner
//         let pruner = PrunerBuilder::new(PruneConfig::default())
//             .build_with_provider_factory(provider.clone());

//         // Create an empty canonical in memory state
//         let canonical_in_memory_state = CanonicalInMemoryState::empty();

//         let task_executor = TaskManager::current();

//         // Start the payload builder service
//         let payload_handle = {
//             let payload_builder =
// EthereumPayloadBuilder::new(EthEvmConfig::new(ctx.chain_spec()));

//             let payload_generator = BasicPayloadJobGenerator::with_builder(
//                 ctx.provider().clone(),
//                 pool,
//                 ctx.task_executor().clone(),
//                 Default::default(),
//                 payload_builder,
//             );
//             let (payload_service, payload_builder) = PayloadBuilderService::new(
//                 payload_generator,
//                 blockchain_db.canonical_state_stream(),
//             );

//             tokio::spawn(payload_service);

//             payload_builder
//         };

//         // Sync metric channel
//         let (sync_metrics_tx, _) = unbounded_channel();

//         // Engine channel
//         let (to_engine, from_engine) = unbounded_channel();

//         // Launch the LocalEngineService in interval mode
//         let period = Duration::from_secs(1);
//         let mut eth_service = LocalEngineService::new(
//             Arc::new(EthBeaconConsensus::new(provider.chain_spec())),
//             EthExecutorProvider::new(
//                 provider.chain_spec(),
//                 EthEvmConfig::new(provider.chain_spec()),
//             ),
//             provider.clone(),
//             blockchain_db,
//             pruner,
//             payload_handle,
//             TreeConfig::default(),
//             Box::new(NoopInvalidBlockHook::default()),
//             sync_metrics_tx,
//             to_engine,
//             Box::pin(UnboundedReceiverStream::new(from_engine)),
//             MiningMode::interval(period),
//             LocalPayloadAttributesBuilder::new(provider.chain_spec()),
//         );

//         // Check that we have no block for now
//         let block = provider.block_by_number(1)?;
//         assert!(block.is_none());

//         // Wait 4 intervals
//         let mut interval = Box::pin(tokio::time::sleep(4 * period));
//         loop {
//             tokio::select! {
//                 ev = eth_service.next() => {},
//                 _ = &mut interval => break,
//             }
//         }

//         // Assert a block has been build
//         let block = provider.block_by_number(1)?;
//         assert!(block.is_some());

//         Ok(())
//     }

//     #[tokio::test]
//     async fn test_local_engine_service_instant() -> eyre::Result<()> {
//         reth_tracing::init_test_tracing();

//         // Start the provider and the pruner
//         let (_, static_dir_path) = create_test_static_files_dir();
//         let provider = ProviderFactory::<NodeTypesWithDBAdapter<TestNode, _>>::new(
//             create_test_rw_db(),
//             MAINNET.clone(),
//             StaticFileProvider::read_write(static_dir_path)?,
//         );
//         let pruner = PrunerBuilder::new(PruneConfig::default())
//             .build_with_provider_factory(provider.clone());

//         // Create an empty canonical in memory state
//         let canonical_in_memory_state = CanonicalInMemoryState::empty();

//         // Start the payload builder service
//         let payload_handle = spawn_test_payload_service::<EthEngineTypes>();

//         // Start a transaction pool
//         let pool = testing_pool();

//         // Sync metric channel
//         let (sync_metrics_tx, _) = unbounded_channel();

//         // Launch the LocalEngineService in instant mode
//         LocalEngineService::spawn_new(
//             payload_handle,
//             TestPayloadAttributesBuilder,
//             provider.clone(),
//             pruner,
//             canonical_in_memory_state,
//             sync_metrics_tx,
//             MiningMode::instant(pool.clone()),
//         );

//         // Wait for a small period to assert block building is
//         // triggered by adding a transaction to the pool
//         let period = Duration::from_millis(500);
//         tokio::time::sleep(period).await;
//         let block = provider.block_by_number(0)?;
//         assert!(block.is_none());

//         // Add a transaction to the pool
//         let transaction = MockTransaction::legacy().with_gas_price(10);
//         pool.add_transaction(Default::default(), transaction).await?;

//         // Wait for block building
//         let period = Duration::from_secs(2);
//         tokio::time::sleep(period).await;

//         // Assert a block has been build
//         let block = provider.block_by_number(0)?;
//         assert!(block.is_some());

//         Ok(())
//     }

//     #[tokio::test]
//     async fn test_canonical_chain_subscription() -> eyre::Result<()> {
//         reth_tracing::init_test_tracing();

//         // Start the provider and the pruner
//         let (_, static_dir_path) = create_test_static_files_dir();
//         let provider = ProviderFactory::<NodeTypesWithDBAdapter<TestNode, _>>::new(
//             create_test_rw_db(),
//             MAINNET.clone(),
//             StaticFileProvider::read_write(static_dir_path)?,
//         );
//         let pruner = PrunerBuilder::new(PruneConfig::default())
//             .build_with_provider_factory(provider.clone());

//         // Create an empty canonical in memory state
//         let canonical_in_memory_state = CanonicalInMemoryState::empty();
//         let mut notifications = canonical_in_memory_state.subscribe_canon_state();

//         // Start the payload builder service
//         let payload_handle = spawn_test_payload_service::<EthEngineTypes>();

//         // Start a transaction pool
//         let pool = testing_pool();

//         // Sync metric channel
//         let (sync_metrics_tx, _) = unbounded_channel();

//         // Launch the LocalEngineService in instant mode
//         LocalEngineService::spawn_new(
//             payload_handle,
//             TestPayloadAttributesBuilder,
//             provider.clone(),
//             pruner,
//             canonical_in_memory_state,
//             sync_metrics_tx,
//             MiningMode::instant(pool.clone()),
//         );

//         // Add a transaction to the pool
//         let transaction = MockTransaction::legacy().with_gas_price(10);
//         pool.add_transaction(Default::default(), transaction).await?;

//         // Check a notification is received for block 0
//         let res = notifications.recv().await?;

//         assert_eq!(res.tip().number, 0);

//         Ok(())
//     }
// }
