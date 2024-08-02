use crate::{mode::MiningMode, Storage};
use futures_util::{future::BoxFuture, FutureExt};
use reth_beacon_consensus::{BeaconEngineMessage, ForkchoiceStatus};
use reth_chainspec::ChainSpec;
use reth_engine_primitives::EngineTypes;
use reth_evm::execute::BlockExecutorProvider;
use reth_primitives::IntoRecoveredTransaction;
use reth_provider::{CanonChainTracker, StateProviderFactory};
use reth_rpc_types::engine::ForkchoiceState;
use reth_stages_api::PipelineEvent;
use reth_tokio_util::EventStream;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tracing::{debug, error, warn};

/// A Future that listens for new ready transactions and puts new blocks into storage
pub struct MiningTask<Client, Pool: TransactionPool, Executor, Engine: EngineTypes> {
    /// The configured chain spec
    chain_spec: Arc<ChainSpec>,
    /// The client used to interact with the state
    client: Client,
    /// The active miner
    miner: MiningMode,
    /// Single active future that inserts a new block into `storage`
    insert_task: Option<BoxFuture<'static, Option<EventStream<PipelineEvent>>>>,
    /// Shared storage to insert new blocks
    storage: Storage,
    /// Pool where transactions are stored
    pool: Pool,
    /// backlog of sets of transactions ready to be mined
    queued: VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
    // TODO: ideally this would just be a sender of hashes
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    /// The pipeline events to listen on
    pipe_line_events: Option<EventStream<PipelineEvent>>,
    /// The type used for block execution
    block_executor: Executor,
}

// === impl MiningTask ===

impl<Executor, Client, Pool: TransactionPool, Engine: EngineTypes>
    MiningTask<Client, Pool, Executor, Engine>
{
    /// Creates a new instance of the task
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        miner: MiningMode,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
        storage: Storage,
        client: Client,
        pool: Pool,
        block_executor: Executor,
    ) -> Self {
        Self {
            chain_spec,
            client,
            miner,
            insert_task: None,
            storage,
            pool,
            to_engine,
            queued: Default::default(),
            pipe_line_events: None,
            block_executor,
        }
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: EventStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<Executor, Client, Pool, Engine> Future for MiningTask<Client, Pool, Executor, Engine>
where
    Client: StateProviderFactory + CanonChainTracker + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    Engine: EngineTypes,
    Executor: BlockExecutorProvider,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // this drives block production and
        loop {
            if let Poll::Ready(transactions) = this.miner.poll(&this.pool, cx) {
                // miner returned a set of transaction that we feed to the producer
                this.queued.push_back(transactions);
            }

            if this.insert_task.is_none() {
                if this.queued.is_empty() {
                    // nothing to insert
                    break
                }

                // ready to queue in new insert task
                let storage = this.storage.clone();
                let transactions = this.queued.pop_front().expect("not empty");

                let to_engine = this.to_engine.clone();
                let client = this.client.clone();
                let chain_spec = Arc::clone(&this.chain_spec);
                let pool = this.pool.clone();
                let events = this.pipe_line_events.take();
                let executor = this.block_executor.clone();

                // Create the mining future that creates a block, notifies the engine that drives
                // the pipeline
                this.insert_task = Some(Box::pin(async move {
                    let mut storage = storage.write().await;

                    let transactions: Vec<_> = transactions
                        .into_iter()
                        .map(|tx| {
                            let recovered = tx.to_recovered_transaction();
                            recovered.into_signed()
                        })
                        .collect();
                    let ommers = vec![];

                    match storage.build_and_execute(
                        transactions.clone(),
                        ommers.clone(),
                        &client,
                        chain_spec,
                        &executor,
                    ) {
                        Ok((new_header, _bundle_state)) => {
                            // clear all transactions from pool
                            pool.remove_transactions(
                                transactions.iter().map(|tx| tx.hash()).collect(),
                            );

                            let state = ForkchoiceState {
                                head_block_hash: new_header.hash(),
                                finalized_block_hash: new_header.hash(),
                                safe_block_hash: new_header.hash(),
                            };
                            drop(storage);

                            // TODO: make this a future
                            // await the fcu call rx for SYNCING, then wait for a VALID response
                            loop {
                                // send the new update to the engine, this will trigger the engine
                                // to download and execute the block we just inserted
                                let (tx, rx) = oneshot::channel();
                                let _ = to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
                                    state,
                                    payload_attrs: None,
                                    tx,
                                });
                                debug!(target: "consensus::auto", ?state, "Sent fork choice update");

                                match rx.await.unwrap() {
                                    Ok(fcu_response) => {
                                        match fcu_response.forkchoice_status() {
                                            ForkchoiceStatus::Valid => break,
                                            ForkchoiceStatus::Invalid => {
                                                error!(target: "consensus::auto", ?fcu_response, "Forkchoice update returned invalid response");
                                                return None
                                            }
                                            ForkchoiceStatus::Syncing => {
                                                debug!(target: "consensus::auto", ?fcu_response, "Forkchoice update returned SYNCING, waiting for VALID");
                                                // wait for the next fork choice update
                                                continue
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        error!(target: "consensus::auto", %err, "Autoseal fork choice update failed");
                                        return None
                                    }
                                }
                            }

                            // update canon chain for rpc
                            client.set_canonical_head(new_header.clone());
                            client.set_safe(new_header.clone());
                            client.set_finalized(new_header.clone());
                        }
                        Err(err) => {
                            warn!(target: "consensus::auto", %err, "failed to execute block")
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
                        break
                    }
                }
            }
        }

        Poll::Pending
    }
}

impl<Client, Pool: TransactionPool, EvmConfig: std::fmt::Debug, Engine: EngineTypes> std::fmt::Debug
    for MiningTask<Client, Pool, EvmConfig, Engine>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiningTask").finish_non_exhaustive()
    }
}
