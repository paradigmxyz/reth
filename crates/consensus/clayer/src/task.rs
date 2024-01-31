use crate::consensus::{
    clayer_block_from_genesis, ClayerConsensusMessagingAgent, PbftConfig, PbftError, PbftMode,
    PbftState,
};
use crate::engine_api::{
    forkchoice_updated, forkchoice_updated_with_attributes, new_payload, ApiService,
};
use crate::engine_pbft::{handle_consensus_event, parse_consensus_message, ConsensusEvent};
use crate::{consensus::ClayerConsensusEngine, engine_api::http::HttpJsonRpc, timing, ClStorage};
use alloy_primitives::B256;
use futures_util::{future::BoxFuture, FutureExt};
use rand::Rng;
use reth_interfaces::clayer::{ClayerConsensusEvent, ClayerConsensusMessageAgentTrait};
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::{
    collections::VecDeque,
    future::Future,
    ops::Add,
    pin::Pin,
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
    block_publishing_ticker: timing::AsyncTicker,
    ///
    network: NetworkHandle,
    ///
    consensus_agent: ClayerConsensusMessagingAgent,
    ///
    storages: CDB,
    pbft_config: PbftConfig,
    pbft_state: Arc<parking_lot::RwLock<PbftState>>,
    pbft_running_state: Arc<AtomicBool>,
    startup_latest_header: SealedHeader,
    consensus_engine_task_handle: Option<std::thread::JoinHandle<()>>,
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
            block_publishing_ticker: timing::AsyncTicker::new(Duration::from_secs(12)),
            network,
            consensus_agent,
            storages,
            pbft_config,
            pbft_state: Arc::new(parking_lot::RwLock::new(pbft_state)),
            pbft_running_state: Arc::new(AtomicBool::new(false)),
            startup_latest_header,
            consensus_engine_task_handle: None,
        }
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }

    pub fn start_clayer_consensus_engine(&mut self) {
        let consensus_agent = self.consensus_agent.clone();
        let api = self.api.clone();
        let mut consensus_engine =
            ClayerConsensusEngine::new(self.consensus_agent.clone(), ApiService::new(api));
        let pbft_state = self.pbft_state.clone();
        let pbft_config = self.pbft_config.clone();

        let startup_latest_header = self.startup_latest_header.clone();
        let thread_join_handle = std::thread::spawn(move || {
            let state = &mut *pbft_state.write();

            let mut block_publishing_ticker =
                timing::SyncTicker::new(pbft_config.block_publishing_delay);

            consensus_engine.initialize(
                clayer_block_from_genesis(&startup_latest_header),
                &pbft_config,
                state,
            );

            consensus_engine.start_idle_timeout(state);

            loop {
                while let Some(event) = consensus_agent.pop_event() {
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
                        match handle_consensus_event(&mut consensus_engine, incoming_event, state) {
                            Ok(again) => {
                                if !again {
                                    break;
                                }
                            }
                            Err(err) => log_any_error(Err(err)),
                        }
                    }
                }

                // If the block publishing delay has passed, attempt to publish a block
                block_publishing_ticker.tick(|| log_any_error(consensus_engine.try_publish(state)));

                // If the idle timeout has expired, initiate a view change
                if consensus_engine.check_idle_timeout_expired(state) {
                    warn!("Idle timeout expired; proposing view change");
                    log_any_error(consensus_engine.start_view_change(state, state.view + 1));
                }

                // If the commit timeout has expired, initiate a view change
                if consensus_engine.check_commit_timeout_expired(state) {
                    warn!("Commit timeout expired; proposing view change");
                    log_any_error(consensus_engine.start_view_change(state, state.view + 1));
                }

                // Check the view change timeout if the node is view changing so we can start a new
                // view change if we don't get a NewView in time
                if let PbftMode::ViewChanging(v) = state.mode {
                    if consensus_engine.check_view_change_timeout_expired(state) {
                        warn!(
                            "View change timeout expired; proposing view change for view {}",
                            v + 1
                        );
                        log_any_error(consensus_engine.start_view_change(state, v + 1));
                    }
                }
            }
        });
        self.consensus_engine_task_handle = Some(thread_join_handle);
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

            // let mut rng = rand::thread_rng();
            // let cn = rng.gen();
            // let hash = B256::with_last_byte(cn);

            // match this.storages.save_consensus_number(hash, cn as u64) {
            //     Ok(o) => {
            //         info!(target:"consensus::cl","trace-consensus ~~~~~~~~~ storages set{}: {}-{}", cn, hash, cn);
            //         if o {
            //             info!(target:"consensus::cl","trace-consensus ~~~~~~~~~ storages set{}: ture", cn);
            //         } else {
            //             info!(target:"consensus::cl","trace-consensus ~~~~~~~~~ storages set{}: false", cn);
            //         }
            //     }
            //     Err(e) => {
            //         info!(target:"consensus::cl","trace-consensus ~~~~~~~~~ storages set{}: error!", cn)
            //     }
            // }

            // if this.storages.consensus_number(hash).is_ok() {
            //     if let Some(num) = this.storages.consensus_number(hash).unwrap() {
            //         info!(target:"consensus::cl","trace-consensus ~~~~~~~~~ storages get{}: {}-{}",cn, hash, num);
            //     } else {
            //         info!(target:"consensus::cl","trace-consensus ~~~~~~~~~ storages get{}: NOne", cn);
            //     }
            // } else {
            //     info!(target:"consensus::cl","trace-consensus ~~~~~~~~~ received get{}: error!", cn);
            // }

            //=========================================================================================
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

                // let mut pbft_running_state = this.pbft_running_state;
                // let pbft_config = this.pbft_config.clone();
                // let mut pbft_state = this.pbft_state.clone();
                // let startup_latest_header = this.startup_latest_header.clone();

                // define task
                this.insert_task = Some(Box::pin(async move {
                    // let mut storage = storage.write().await;
                    // let last_block_hash = storage.best_hash.clone();
                    // let last_block_height = storage.best_height;

                    // info!(target: "consensus::cl","step 1: forkchoice_updated {}",timestamp);
                    // let forkchoice_updated_result = match forkchoice_updated(
                    //     &api,
                    //     last_block_hash.clone(),
                    // )
                    // .await
                    // {
                    //     Ok(x) => x,
                    //     Err(e) => {
                    //         error!(target:"consensus::cl", "step 1: Forkchoice updated error: {:?}", e);
                    //         return events;
                    //     }
                    // };
                    // info!(target: "consensus::cl","forkchoice state response {:?}", forkchoice_updated_result);
                    // if !forkchoice_updated_result.payload_status.status.is_valid() {
                    //     return events;
                    // }

                    // info!(target: "consensus::cl","step 2: forkchoice_updated_with_attributes");
                    // let forkchoice_updated_result = match forkchoice_updated_with_attributes(
                    //     &api,
                    //     last_block_hash.clone(),
                    // )
                    // .await
                    // {
                    //     Ok(x) => x,
                    //     Err(e) => {
                    //         error!(target:"consensus::cl", "step 2: Forkchoice updated error: {:?}", e);
                    //         return events;
                    //     }
                    // };

                    // info!(target: "consensus::cl","forkchoice state response {:?}", forkchoice_updated_result);
                    // if !forkchoice_updated_result.payload_status.status.is_valid() {
                    //     return events;
                    // }

                    // let execution_payload = match forkchoice_updated_result.payload_id {
                    //     Some(id) => {
                    //         info!(target: "consensus::cl","step 3: get_payload");
                    //         match api.get_payload_v2(id).await {
                    //             Ok(x) => x,
                    //             Err(e) => {
                    //                 error!(target:"consensus::cl", "step 3: Get payload error: {:?}", e);
                    //                 return events;
                    //             }
                    //         }
                    //     }
                    //     None => {
                    //         return events;
                    //     }
                    // };
                    // info!(target: "consensus::cl","execution payload {:?}", execution_payload);
                    // let newest_height =
                    //     execution_payload.execution_payload.payload_inner.block_number;

                    // let payload_status = match new_payload(&api, execution_payload).await {
                    //     Ok(x) => {
                    //         info!(target: "consensus::cl","step 4: new_payload");
                    //         x
                    //     }
                    //     Err(e) => {
                    //         error!(target:"consensus::cl", "step 4: New payload error: {:?}", e);
                    //         return events;
                    //     }
                    // };
                    // info!(target: "consensus::cl","step 4: payload status {:?}", payload_status);
                    // if !payload_status.status.is_valid()
                    //     || payload_status.latest_valid_hash.is_none()
                    // {
                    //     error!(target:"consensus::cl", "step 4: Payload status not valid");
                    //     return events;
                    // }

                    // if let Some(latest_valid_hash) = &payload_status.latest_valid_hash {
                    //     info!(target: "consensus::cl","step 5: forkchoice_updated");
                    //     let forkchoice_updated_result: ForkchoiceUpdated = match forkchoice_updated(
                    //         &api,
                    //         latest_valid_hash.clone(),
                    //     )
                    //     .await
                    //     {
                    //         Ok(x) => x,
                    //         Err(e) => {
                    //             error!(target:"consensus::cl", "Forkchoice updated error: {:?}", e);
                    //             return events;
                    //         }
                    //     };
                    //     info!(target: "consensus::cl","forkchoice state response {:?}", forkchoice_updated_result);

                    //     if forkchoice_updated_result.payload_status.status.is_valid() {
                    //         storage.best_hash = latest_valid_hash.clone();
                    //         storage.best_height = newest_height;
                    //     } else {
                    //         error!(target:"consensus::cl", "Forkchoice not valid", );
                    //         return events;
                    //     }
                    // }
                    // info!(target: "consensus::cl","step end");

                    events
                }));
            }
            //consensu.init();

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
