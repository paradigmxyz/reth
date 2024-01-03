use crate::{consensus::ClayerConsensusEngine, engine_api::http::HttpJsonRpc, timing, ClStorage};
use alloy_primitives::B256;
use futures_util::{future::BoxFuture, FutureExt};
use reth_interfaces::clayer::ClayerConsensus;
use reth_network::NetworkHandle;
use reth_primitives::hex;
use reth_primitives::{Block, ChainSpec, IntoRecoveredTransaction, SealedBlockWithSenders};
use reth_provider::{CanonChainTracker, CanonStateNotificationSender, Chain, StateProviderFactory};
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
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

pub struct ClTask<Client, Pool: TransactionPool> {
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
    consensus_engine: ClayerConsensusEngine,
}

impl<Client, Pool: TransactionPool> ClTask<Client, Pool> {
    /// Creates a new instance of the task
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        storage: ClStorage,
        client: Client,
        pool: Pool,
        api: Arc<HttpJsonRpc>,
        network: NetworkHandle,
        consensus_engine: ClayerConsensusEngine,
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
            consensus_engine,
        }
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<Client, Pool> Future for ClTask<Client, Pool>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        'first_layer: loop {
            if let Poll::Ready(x) = this.block_publishing_ticker.poll(cx) {
                info!(target:"consensus::cl", "Attempting publish block");
                this.queued.push_back(x);
            }

            if let Some(data) = this.consensus_engine.pop_cache() {
                info!(target:"consensus::cl","trace-consensus ========= received consensus: {}",hex::encode(data));
            }

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
                let is_validator = this.consensus_engine.is_validator();
                let network = this.network.clone();
                let consensus_engine = this.consensus_engine.clone();

                // define task
                this.insert_task = Some(Box::pin(async move {
                    let self_id = network.peer_id();
                    info!(target:"consensus::cl","trace-consensus =========  broadcast_consensus: {}",hex::encode(self_id));
                    consensus_engine.broadcast_consensus(reth_primitives::Bytes::copy_from_slice(
                        self_id.as_slice(),
                    ));
                    if !is_validator {
                        return events;
                    }

                    let mut storage = storage.write().await;
                    let last_block_hash = storage.best_hash.clone();
                    let last_block_height = storage.best_height;

                    info!(target: "consensus::cl","step 1: forkchoice_updated {}",timestamp);
                    let forkchoice_updated_result = match ClayerConsensusEngine::forkchoice_updated(
                        &api,
                        last_block_hash.clone(),
                    )
                    .await
                    {
                        Ok(x) => x,
                        Err(e) => {
                            error!(target:"consensus::cl", "step 1: Forkchoice updated error: {:?}", e);
                            return events;
                        }
                    };
                    info!(target: "consensus::cl","forkchoice state response {:?}", forkchoice_updated_result);
                    if !forkchoice_updated_result.payload_status.status.is_valid() {
                        return events;
                    }

                    info!(target: "consensus::cl","step 2: forkchoice_updated_with_attributes");
                    let forkchoice_updated_result =
                        match ClayerConsensusEngine::forkchoice_updated_with_attributes(
                            &api,
                            last_block_hash.clone(),
                        )
                        .await
                        {
                            Ok(x) => x,
                            Err(e) => {
                                error!(target:"consensus::cl", "step 2: Forkchoice updated error: {:?}", e);
                                return events;
                            }
                        };

                    info!(target: "consensus::cl","forkchoice state response {:?}", forkchoice_updated_result);
                    if !forkchoice_updated_result.payload_status.status.is_valid() {
                        return events;
                    }

                    let execution_payload = match forkchoice_updated_result.payload_id {
                        Some(id) => {
                            info!(target: "consensus::cl","step 3: get_payload");
                            match api.get_payload_v2(id).await {
                                Ok(x) => x,
                                Err(e) => {
                                    error!(target:"consensus::cl", "step 3: Get payload error: {:?}", e);
                                    return events;
                                }
                            }
                        }
                        None => {
                            return events;
                        }
                    };
                    info!(target: "consensus::cl","execution payload {:?}", execution_payload);
                    let newest_height =
                        execution_payload.execution_payload.payload_inner.block_number;

                    let payload_status = match ClayerConsensusEngine::new_payload(
                        &api,
                        execution_payload,
                    )
                    .await
                    {
                        Ok(x) => {
                            info!(target: "consensus::cl","step 4: new_payload");
                            x
                        }
                        Err(e) => {
                            error!(target:"consensus::cl", "step 4: New payload error: {:?}", e);
                            return events;
                        }
                    };
                    info!(target: "consensus::cl","step 4: payload status {:?}", payload_status);
                    if !payload_status.status.is_valid()
                        || payload_status.latest_valid_hash.is_none()
                    {
                        error!(target:"consensus::cl", "step 4: Payload status not valid");
                        return events;
                    }

                    if let Some(latest_valid_hash) = &payload_status.latest_valid_hash {
                        info!(target: "consensus::cl","step 5: forkchoice_updated");
                        let forkchoice_updated_result: ForkchoiceUpdated =
                            match ClayerConsensusEngine::forkchoice_updated(
                                &api,
                                latest_valid_hash.clone(),
                            )
                            .await
                            {
                                Ok(x) => x,
                                Err(e) => {
                                    error!(target:"consensus::cl", "Forkchoice updated error: {:?}", e);
                                    return events;
                                }
                            };
                        info!(target: "consensus::cl","forkchoice state response {:?}", forkchoice_updated_result);

                        if forkchoice_updated_result.payload_status.status.is_valid() {
                            storage.best_hash = latest_valid_hash.clone();
                            storage.best_height = newest_height;
                        } else {
                            error!(target:"consensus::cl", "Forkchoice not valid", );
                            return events;
                        }
                    }

                    info!(target: "consensus::cl","step end");
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
