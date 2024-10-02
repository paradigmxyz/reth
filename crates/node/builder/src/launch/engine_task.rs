use alloy_primitives::B256;
use alloy_rpc_types::engine::{CancunPayloadFields, PayloadStatus};
use futures::{
    stream::{Fuse, FuturesUnordered},
    StreamExt,
};
use reth_beacon_consensus::{
    BeaconConsensusEngineEvent, BeaconEngineMessage, BeaconOnNewPayloadError, EngineNodeTypes,
};
use reth_chain_state::ExecutedBlock;
use reth_chainspec::{EthereumHardforks, Head};
use reth_engine_service::service::EngineService;
use reth_engine_tree::{
    chain::ChainEvent,
    engine::{EngineApiRequest, EngineRequestHandler},
};
use reth_evm::execute::BlockExecutorProvider;
use reth_network::SyncState;
use reth_network_api::FullNetwork;
use reth_network_p2p::BlockClient;
use reth_node_api::{BuiltPayload, NodeTypes};
use reth_node_core::rpc::compat::engine::payload::block_to_payload;
use reth_payload_primitives::BuiltPayloadStream;
use reth_tokio_util::EventSender;
use reth_tracing::tracing::{debug, error, info};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::oneshot;

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub(crate) struct EngineServiceTask<N, Client, E, Network>
where
    N: EngineNodeTypes,
    Client: BlockClient + 'static,
    E: BlockExecutorProvider + 'static,
{
    chain_spec: Arc<<N as NodeTypes>::ChainSpec>,
    network_handle: Network,
    built_payload_stream: Fuse<BuiltPayloadStream<N::Engine>>,
    service: EngineService<N, Client, E>,
    event_sender: EventSender<BeaconConsensusEngineEvent>,
    initial_backfill_target: Option<B256>,
    payload_validation_enabled: bool,
    new_payload_responses:
        FuturesUnordered<oneshot::Receiver<Result<PayloadStatus, BeaconOnNewPayloadError>>>,
}

impl<N, Client, E, Network> EngineServiceTask<N, Client, E, Network>
where
    N: EngineNodeTypes,
    Client: BlockClient + 'static,
    E: BlockExecutorProvider + 'static,
    Network: FullNetwork,
{
    pub(crate) fn new(
        chain_spec: Arc<<N as NodeTypes>::ChainSpec>,
        network_handle: Network,
        built_payload_stream: Fuse<BuiltPayloadStream<N::Engine>>,
        service: EngineService<N, Client, E>,
        event_sender: EventSender<BeaconConsensusEngineEvent>,
        initial_backfill_target: Option<B256>,
        payload_validation_enabled: bool,
    ) -> Self {
        Self {
            chain_spec,
            network_handle,
            built_payload_stream,
            service,
            event_sender,
            initial_backfill_target,
            payload_validation_enabled,
            new_payload_responses: FuturesUnordered::default(),
        }
    }

    fn on_chain_event(&self, event: ChainEvent<BeaconConsensusEngineEvent>) -> eyre::Result<()> {
        debug!(target: "reth::cli", "Event: {event}");
        match event {
            ChainEvent::BackfillSyncFinished => {
                self.network_handle.update_sync_state(SyncState::Idle);
            }
            ChainEvent::BackfillSyncStarted => {
                self.network_handle.update_sync_state(SyncState::Syncing);
            }
            ChainEvent::FatalError => {
                error!(target: "reth::cli", "Fatal error in consensus engine");
                return Err(eyre::eyre!("Fatal error in consensus engine"));
            }
            ChainEvent::Handler(ev) => {
                if let Some(head) = ev.canonical_header() {
                    let head_block = Head {
                        number: head.number,
                        hash: head.hash(),
                        difficulty: head.difficulty,
                        timestamp: head.timestamp,
                        total_difficulty: self
                            .chain_spec
                            .final_paris_total_difficulty(head.number)
                            .unwrap_or_default(),
                    };
                    self.network_handle.update_status(head_block);
                }
                self.event_sender.notify(ev);
            }
        }
        Ok(())
    }

    fn on_built_executed_block(&mut self, executed_block: ExecutedBlock) {
        let request = if self.payload_validation_enabled {
            debug!(target: "reth::cli", block=?executed_block.block().num_hash(), "inserting built payload with validation");
            let (tx, rx) = oneshot::channel();
            self.new_payload_responses.push(rx);

            let message = BeaconEngineMessage::NewPayload {
                payload: block_to_payload(executed_block.block.as_ref().clone()),
                cancun_fields: executed_block.block.parent_beacon_block_root.map(
                    |parent_beacon_block_root| CancunPayloadFields {
                        parent_beacon_block_root,
                        versioned_hashes: executed_block
                            .block
                            .blob_versioned_hashes_iter()
                            .copied()
                            .collect(),
                    },
                ),
                tx,
            };
            EngineApiRequest::Beacon(message)
        } else {
            debug!(target: "reth::cli", block=?executed_block.block().num_hash(), "inserting built payload without validation");
            EngineApiRequest::InsertExecutedBlock(executed_block)
        };

        self.service.orchestrator_mut().handler_mut().handler_mut().on_event(request.into());
    }

    fn on_new_payload_response(
        &self,
        response: Result<Result<PayloadStatus, BeaconOnNewPayloadError>, oneshot::error::RecvError>,
    ) {
        match response {
            Ok(Ok(PayloadStatus { status, latest_valid_hash })) => {
                if status.is_invalid() {
                    error!(target: "reth::cli", ?status, ?latest_valid_hash, "Invalid built payload");
                } else {
                    info!(target: "reth::cli", ?status, ?latest_valid_hash, "Received response for built payload");
                }
            }
            Ok(Err(error)) => {
                error!(target: "reth::cli", %error, "Error validating newly built payload");
            }
            Err(_error) => {
                error!(target: "reth::cli", "Channel for built payload result closed");
            }
        };
    }
}

impl<N, Client, E, Network> Future for EngineServiceTask<N, Client, E, Network>
where
    N: EngineNodeTypes,
    Client: BlockClient + 'static,
    E: BlockExecutorProvider + 'static,
    Network: FullNetwork + Unpin,
{
    type Output = eyre::Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // start backfill if initial target was provided
        if let Some(initial_target) = this.initial_backfill_target.take() {
            debug!(target: "reth::cli", %initial_target,  "start backfill sync");
            this.service.orchestrator_mut().start_backfill_sync(initial_target);
        }

        loop {
            // poll engine service
            if let Poll::Ready(next) = this.service.poll_next_unpin(cx) {
                if let Some(event) = next {
                    this.on_chain_event(event)?;
                    continue
                }

                // engine service stream has closed
                return Poll::Ready(Ok(()))
            }

            // poll built payload stream
            if let Poll::Ready(Some(payload)) = this.built_payload_stream.poll_next_unpin(cx) {
                if let Some(block) = payload.executed_block() {
                    this.on_built_executed_block(block);
                }
                continue
            }

            // poll responses for forwarded new payloads
            if let Poll::Ready(Some(response)) = this.new_payload_responses.poll_next_unpin(cx) {
                this.on_new_payload_response(response);
                continue
            }

            return Poll::Pending
        }
    }
}
