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
use reth_beacon_consensus::{BeaconConsensusEngineEvent, EngineNodeTypes};
use reth_chainspec::EthChainSpec;
use reth_consensus::Consensus;
use reth_engine_primitives::BeaconEngineMessage;
use reth_engine_service::service::EngineMessageStream;
use reth_engine_tree::{
    chain::{ChainEvent, HandlerEvent},
    engine::{
        EngineApiKind, EngineApiRequest, EngineApiRequestHandler, EngineRequestHandler, FromEngine,
        RequestHandlerEvent,
    },
    persistence::{PersistenceHandle, PersistenceNodeTypes},
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
    N: EngineNodeTypes + PersistenceNodeTypes,
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
            PersistenceHandle::spawn_service(provider, pruner, sync_metrics_tx);
        let payload_validator = ExecutionPayloadValidator::new(chain_spec);

        let canonical_in_memory_state = blockchain_db.canonical_in_memory_state();

        let (to_tree_tx, from_tree) = EngineApiTreeHandler::<N::Primitives, _, _, _, _>::spawn_new(
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
