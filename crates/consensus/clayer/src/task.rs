use crate::consensus::{
    assemble_peer_id, clayer_block_from_header, clayer_block_from_seal,
    ClayerConsensusMessagingAgent, PbftConfig, PbftError, PbftMode, PbftState,
};

use crate::engine_api::ApiService;
use crate::engine_pbft::{handle_consensus_event, parse_consensus_message, ConsensusEvent};
use crate::{
    consensus::{ClayerConsensusEngine, ELECT_VOTING_ADDRESS},
    timing,
};
use crate::{create_sync_api, AuthHttpConfig};
use futures_util::{future::BoxFuture, FutureExt};
use reth_interfaces::clayer::{ClayerConsensusEvent, ClayerConsensusMessageAgentTrait};
use reth_network::NetworkHandle;
use reth_primitives::{ChainSpec, SealedHeader};
use reth_provider::{
    BlockReaderIdExt, CanonChainTracker, ConsensusNumberReader, ConsensusNumberWriter,
    StateProviderFactory,
};
use reth_stages::PipelineEvent;
use secp256k1::SecretKey;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

pub struct ClTask<Client, CDB> {
    /// The configured chain spec
    chain_spec: Arc<ChainSpec>,
    /// The client used to interact with the state
    client: Client,
    /// Single active future that inserts a new block into `storage`
    insert_task: Option<BoxFuture<'static, Option<UnboundedReceiverStream<PipelineEvent>>>>,
    /// backlog of sets of transactions ready to be mined
    // queued: VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
    queued: VecDeque<u64>,
    /// The pipeline events to listen on
    pipe_line_events: Option<UnboundedReceiverStream<PipelineEvent>>,
    ///
    block_publishing_ticker: timing::AsyncTicker,
    ///
    network: NetworkHandle,
    ///
    consensus_agent: ClayerConsensusMessagingAgent,
    ///
    storages: Arc<CDB>,
    pbft_running_state: Arc<AtomicBool>,
    startup_latest_header: SealedHeader,
    consensus_engine_task_handle: Option<std::thread::JoinHandle<()>>,
    auth_config: AuthHttpConfig,
    secret: SecretKey,
}

impl<Client, CDB> ClTask<Client, CDB>
where
    CDB: ConsensusNumberReader + ConsensusNumberWriter + 'static,
    Client: BlockReaderIdExt + Clone + 'static,
{
    /// Creates a new instance of the task
    pub(crate) fn new(
        secret: SecretKey,
        chain_spec: Arc<ChainSpec>,
        client: Client,
        auth_config: AuthHttpConfig,
        network: NetworkHandle,
        consensus_agent: ClayerConsensusMessagingAgent,
        storages: CDB,
        startup_latest_header: SealedHeader,
    ) -> Self {
        Self {
            secret,
            chain_spec,
            client,
            insert_task: None,
            queued: Default::default(),
            pipe_line_events: None,
            auth_config,
            block_publishing_ticker: timing::AsyncTicker::new(Duration::from_secs(30)),
            network,
            consensus_agent,
            storages: Arc::new(storages),
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
        let auth_config = self.auth_config.clone();

        let cdb = self.storages.clone();
        let client = self.client.clone();
        let secret = self.secret.clone();

        let startup_latest_header = self.startup_latest_header.clone();
        let thread_join_handle = std::thread::spawn(move || {
            let api = create_sync_api(&auth_config);
            let execution_block =
                api.get_block_by_number("latest".to_string()).expect("get latest block error");
            info!(target: "consensus::cl","latest block: {:?}", execution_block);
            let validator_datas = api
                .query_validators(ELECT_VOTING_ADDRESS.to_string(), startup_latest_header.number)
                .expect("query validators failed");
            let peers = assemble_peer_id(validator_datas).expect("parse peer id failed");

            let mut pbft_config = PbftConfig::default();
            pbft_config.members.clone_from(&peers);
            let mut pbft_state = PbftState::new(
                secret,
                startup_latest_header.number,
                startup_latest_header.timestamp,
                &pbft_config,
            );
            let state = &mut pbft_state;
            let mut consensus_engine = ClayerConsensusEngine::new(
                consensus_agent.clone(),
                ApiService::new(Arc::new(api)),
                cdb,
                client,
            );

            // let receiver = consensus_agent.receiver();
            let mut block_publishing_ticker =
                timing::SyncTicker::new(pbft_config.block_publishing_delay);

            let seal = match consensus_engine.load_seal(startup_latest_header.hash) {
                Ok(seal) => seal,
                Err(e) => {
                    log_any_error(Err(e));
                    panic!("Failed to load seal");
                }
            };

            let block = if startup_latest_header.number == 0 {
                // genesis block
                clayer_block_from_header(&startup_latest_header)
            } else {
                match seal {
                    Some(seal) => clayer_block_from_seal(&startup_latest_header, seal),
                    None => {
                        if state.is_validator() {
                            //no seal,so need to sync seal
                            state.becoming_validator = true;
                        }
                        clayer_block_from_header(&startup_latest_header)
                    }
                }
            };
            consensus_engine.initialize(block, &pbft_config, state);
            consensus_engine.start_idle_timeout(state);

            loop {
                if let Some(event) = consensus_agent.pop_event() {
                    let incoming_event = match event {
                        ClayerConsensusEvent::PeerNetWork(peer_id, connect) => {
                            let e = if connect {
                                Some(ConsensusEvent::PeerConnected(peer_id))
                            } else {
                                Some(ConsensusEvent::PeerDisconnected(peer_id))
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
                        ClayerConsensusEvent::BlockValid(block_id) => {
                            Some(ConsensusEvent::BlockValid(block_id))
                        }
                        ClayerConsensusEvent::BlockInvalid(block_id) => {
                            Some(ConsensusEvent::BlockInvalid(block_id))
                        }
                        ClayerConsensusEvent::BlockCommit((block_id, timestamp, committing)) => {
                            Some(ConsensusEvent::BlockCommit((block_id, timestamp, committing)))
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
                } else {
                    log_any_error(consensus_engine.sync_seal(state));
                    sleep(pbft_config.update_recv_timeout);
                }

                if state.is_validator() {
                    // If the block publishing delay has passed, attempt to publish a block
                    block_publishing_ticker
                        .tick(|| log_any_error(consensus_engine.try_publish(state)));

                    // If the idle timeout has expired, initiate a view change
                    if consensus_engine.check_idle_timeout_expired(state) {
                        warn!(target:"consensus::cl", "Idle timeout expired; proposing view change");
                        log_any_error(consensus_engine.start_view_change(state, state.view + 1));
                    }

                    // If the commit timeout has expired, initiate a view change
                    if consensus_engine.check_commit_timeout_expired(state) {
                        warn!(target:"consensus::cl", "Commit timeout expired; proposing view change");
                        log_any_error(consensus_engine.start_view_change(state, state.view + 1));
                    }

                    // Check the view change timeout if the node is view changing so we can start a new
                    // view change if we don't get a NewView in time
                    if let PbftMode::ViewChanging(v) = state.mode {
                        if consensus_engine.check_view_change_timeout_expired(state) {
                            warn!(target:"consensus::cl",
                                "View change timeout expired; proposing view change for view {}",
                                v + 1
                            );
                            log_any_error(consensus_engine.start_view_change(state, v + 1));
                        }
                    }
                }
            }
        });
        self.consensus_engine_task_handle = Some(thread_join_handle);
    }
}

impl<Client, CDB> Future for ClTask<Client, CDB>
where
    Client: StateProviderFactory + CanonChainTracker + BlockReaderIdExt + Clone + Unpin + 'static,
    CDB: ConsensusNumberReader + ConsensusNumberWriter + Unpin + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            if let Poll::Ready(x) = this.block_publishing_ticker.poll(cx) {
                this.queued.push_back(x);

                if !this.pbft_running_state.load(Ordering::Relaxed) {
                    this.pbft_running_state.store(true, Ordering::Relaxed);
                    this.start_clayer_consensus_engine();
                }
            }

            if this.insert_task.is_none() {
                if this.queued.is_empty() {
                    // nothing to insert
                    break;
                }

                let chain_spec = Arc::clone(&this.chain_spec);
                let client = this.client.clone();
                let events = this.pipe_line_events.take();
                let network = this.network.clone();

                // define task
                this.insert_task = Some(Box::pin(async move {
                    let network_id = network.peer_id();
                    tokio::time::sleep(Duration::from_millis(5000)).await;
                    match client.latest_header().ok() {
                        Some(header) => {
                            if let Some(header) = header {
                                trace!(target: "consensus::cl", "execute insert task(chain id: {} network_id: {} last block: {})",chain_spec.chain.id(),network_id,header.number);
                            }
                        }
                        None => {
                            trace!(target: "consensus::cl", "execute insert task(chain id: {} network_id: {}) last block",chain_spec.chain.id(),network_id);
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
