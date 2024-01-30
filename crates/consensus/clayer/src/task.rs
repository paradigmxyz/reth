use crate::consensus::{clayer_block_from_genesis, PbftConfig, PbftError, PbftMode, PbftState};
use crate::engine_api::{
    forkchoice_updated, forkchoice_updated_with_attributes, new_payload, ApiService,
};
use crate::engine_pbft::{handle_consensus_event, parse_consensus_message, ConsensusEvent};
use crate::ClayerConsensusMessagingAgent;
use crate::{consensus::ClayerConsensusEngine, engine_api::http::HttpJsonRpc, timing, ClStorage};
use alloy_primitives::B256;
use futures_util::{future::BoxFuture, FutureExt};
use rand::Rng;
use reth_interfaces::clayer::{ClayerConsensusEvent, ClayerConsensusMessageAgentTrait};
use reth_interfaces::consensus;
use reth_network::NetworkHandle;
use reth_primitives::{hex, SealedHeader, TransactionSigned};
use reth_primitives::{Block, ChainSpec, IntoRecoveredTransaction, SealedBlockWithSenders};
use reth_provider::providers::BlockchainProvider;
use reth_provider::{
    CanonChainTracker, CanonStateNotificationSender, Chain, ConsensusNumberReader,
    ConsensusNumberWriter, StateProviderFactory,
};
use reth_rpc_types::engine::{
    ExecutionPayloadFieldV2, ForkchoiceState, ForkchoiceUpdated, PayloadAttributes,
};
use reth_stages::PipelineEvent;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use std::{
    collections::VecDeque,
    future::Future,
    ops::Add,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

pub const EXECUTE_PBFT: bool = false;

pub struct ClTask<Client, Pool: TransactionPool, CDB> {
    /// The configured chain spec
    chain_spec: Arc<ChainSpec>,
    /// The client used to interact with the state
    client: Client,
    /// Single active future that inserts a new block into `storage`
    insert_task: Option<BoxFuture<'static, Option<UnboundedReceiverStream<PipelineEvent>>>>,
    /// Shared storage to insert new blocks
    storage: ClStorage,
    /// Pool where transactions are stored
    pool: Pool,
    /// backlog of sets of transactions ready to be mined
    // queued: VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
    queued: VecDeque<u64>,
    /// The pipeline events to listen on
    pipe_line_events: Option<UnboundedReceiverStream<PipelineEvent>>,
    /// API
    api: Arc<HttpJsonRpc>,
    ///
    block_publishing_ticker: timing::Ticker,
    ///
    network: NetworkHandle,
    ///
    consensus_agent: ClayerConsensusMessagingAgent,
    ///
    consensus_engine: Arc<tokio::sync::RwLock<ClayerConsensusEngine>>,
    storages: CDB,
    pbft_config: PbftConfig,
    pbft_running_state: Arc<AtomicBool>,
    startup_latest_header: SealedHeader,
}

impl<Client, Pool: TransactionPool, CDB> ClTask<Client, Pool, CDB> {
    /// Creates a new instance of the task
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        storage: ClStorage,
        client: Client,
        pool: Pool,
        api: Arc<HttpJsonRpc>,
        network: NetworkHandle,
        consensus_agent: ClayerConsensusMessagingAgent,
        storages: CDB,
        pbft_config: PbftConfig,
        pbft_state: PbftState,
        startup_latest_header: SealedHeader,
    ) -> Self {
        Self {
            chain_spec,
            client,
            insert_task: None,
            storage,
            pool,
            queued: Default::default(),
            pipe_line_events: None,
            api,
            block_publishing_ticker: timing::Ticker::new(Duration::from_secs(12)),
            network,
            consensus_agent: consensus_agent.clone(),
            consensus_engine: Arc::new(tokio::sync::RwLock::new(ClayerConsensusEngine::new(
                consensus_agent,
                pbft_state.clone(),
            ))), //ClayerConsensusEngine::new(consensus_agent, pbft_state.clone()),
            storages,
            pbft_config,
            pbft_running_state: Arc::new(AtomicBool::new(false)),
            startup_latest_header,
        }
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<Client, Pool, CDB> Future for ClTask<Client, Pool, CDB>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
    CDB: ConsensusNumberReader + ConsensusNumberWriter + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        info!(target:"consensus::cl", "Starting consensus task");

        'first_layer: loop {
            if let Poll::Ready(x) = this.block_publishing_ticker.poll(cx) {
                info!(target:"consensus::cl", "Attempting publish block");
                this.queued.push_back(x);
            }
            // sleep(std::time::Duration::from_millis(100));

            if this.insert_task.is_none() {
                if this.queued.is_empty() {
                    // nothing to insert
                    break;
                }

                let timestamp = this.queued.pop_front().expect("not empty");
                let api = this.api.clone();
                let storage = this.storage.clone();
                let chain_spec = Arc::clone(&this.chain_spec);
                let client = this.client.clone();
                let events = this.pipe_line_events.take();
                let network = this.network.clone();
                let consensus_agent = this.consensus_agent.clone();

                let pbft_running_state = this.pbft_running_state.clone();
                let pbft_config = this.pbft_config.clone();
                let startup_latest_header = this.startup_latest_header.clone();
                let consensus_engine = this.consensus_engine.clone();

                // define task
                this.insert_task = Some(Box::pin(async move {
                    info!(target:"consensus::cl", "processing bpft task");
                    let mut consensus_engine = consensus_engine.write().await;

                    if !pbft_running_state.load(Ordering::Relaxed) {
                        match consensus_engine
                            .initialize(
                                clayer_block_from_genesis(&startup_latest_header),
                                ApiService::new(api.clone()),
                                &pbft_config,
                            )
                            .await
                        {
                            Ok(_) => {
                                pbft_running_state.store(true, Ordering::Relaxed);
                                consensus_engine.start_idle_timeout();
                            }
                            Err(err) => log_any_error(Err(err)),
                        }
                    }

                    if pbft_running_state.load(Ordering::Relaxed) {
                        if let Some(event) = consensus_agent.pop_event() {
                            let incoming_event = match event {
                                ClayerConsensusEvent::PeerNetWork(peer_id, connect) => {
                                    let e = if connect {
                                        Some(ConsensusEvent::PeerConnected(peer_id))
                                    } else {
                                        Some(ConsensusEvent::PeerConnected(peer_id))
                                    };
                                    e
                                }
                                ClayerConsensusEvent::PeerMessage(peer_id, bytes) => {
                                    let e = match parse_consensus_message(&bytes) {
                                        Ok(msg) => Some(ConsensusEvent::PeerMessage(peer_id, msg)),
                                        Err(e) => {
                                            log_any_error(Err(e));
                                            None
                                        }
                                    };
                                    e
                                }
                            };
                            if let Some(incoming_event) = incoming_event {
                                match handle_consensus_event(&mut consensus_engine, incoming_event)
                                    .await
                                {
                                    Ok(again) => {
                                        if !again {
                                            return events;
                                        }
                                    }
                                    Err(err) => log_any_error(Err(err)),
                                }
                            }
                        }

                        // If the block publishing delay has passed, attempt to publish a block
                        if consensus_engine.check_block_publishing_expired() {
                            if let Err(e) = consensus_engine.try_publish().await {
                                log_any_error(Err(e));
                            }
                        }

                        let view = consensus_engine.view();

                        // If the idle timeout has expired, initiate a view change
                        if consensus_engine.check_idle_timeout_expired() {
                            warn!(target:"consensus::cl","Idle timeout expired; proposing view change");
                            log_any_error(consensus_engine.start_view_change(view + 1));
                        }

                        let view = consensus_engine.view();

                        // If the commit timeout has expired, initiate a view change
                        if consensus_engine.check_commit_timeout_expired() {
                            warn!(target:"consensus::cl","Commit timeout expired; proposing view change");
                            log_any_error(consensus_engine.start_view_change(view + 1));
                        }

                        let view = consensus_engine.view();
                        // Check the view change timeout if the node is view changing so we can start a new
                        // view change if we don't get a NewView in time
                        if let PbftMode::ViewChanging(v) = consensus_engine.mode() {
                            if consensus_engine.check_view_change_timeout_expired() {
                                warn!(target:"consensus::cl","View change timeout expired; proposing view change for view {}", v + 1);
                                log_any_error(consensus_engine.start_view_change(view + 1));
                            }
                        }
                    }

                    events
                }));
            }

            if let Some(mut fut) = this.insert_task.take() {
                match fut.poll_unpin(cx) {
                    Poll::Ready(events) => {
                        this.pipe_line_events = events;
                    }
                    Poll::Pending => {
                        this.insert_task = Some(fut);
                        break;
                    }
                }
            }
        }
        Poll::Pending
    }
}

fn log_any_error(res: Result<(), PbftError>) {
    if let Err(e) = res {
        // Treat errors that result from other nodes' messages as warnings
        match e {
            PbftError::SigningError(_)
            | PbftError::FaultyPrimary(_)
            | PbftError::InvalidMessage(_) => warn!("{}", e),
            _ => error!(target:"consensus::cl","{}", e),
        }
    }
}
