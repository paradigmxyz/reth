use crate::{mode::MiningMode, Storage};
use futures_util::{future::BoxFuture, FutureExt, StreamExt};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_interfaces::{consensus::ForkchoiceState, events::NewBlockNotificationSink};
use reth_primitives::{
    constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS},
    proofs, Block, BlockBody, ChainSpec, Header, IntoRecoveredTransaction, ReceiptWithBloom,
    EMPTY_OMMER_ROOT, U256,
};
use reth_provider::StateProviderFactory;
use reth_revm::{
    database::{State, SubState},
    executor::Executor,
};
use reth_stages::{stages::FINISH, PipelineEvent};
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, trace, warn};

/// A Future that listens for new ready transactions and puts new blocks into storage
pub struct MiningTask<Client, Pool: TransactionPool> {
    /// The configured chain spec
    chain_spec: Arc<ChainSpec>,
    /// The client used to interact with the state
    client: Client,
    /// The active miner
    miner: MiningMode,
    /// Single active future that inserts a new block into `storage`
    insert_task: Option<BoxFuture<'static, Option<UnboundedReceiverStream<PipelineEvent>>>>,
    /// Shared storage to insert new blocks
    storage: Storage,
    /// Pool where transactions are stored
    pool: Pool,
    /// backlog of sets of transactions ready to be mined
    queued: VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
    /// TODO: ideally this would just be a sender of hashes
    to_engine: UnboundedSender<BeaconEngineMessage>,
    /// Used to notify consumers of new blocks
    new_block_notification_sender: NewBlockNotificationSink,
    /// The pipeline events to listen on
    pipe_line_events: Option<UnboundedReceiverStream<PipelineEvent>>,
}

// === impl MiningTask ===

impl<Client, Pool: TransactionPool> MiningTask<Client, Pool> {
    /// Creates a new instance of the task
    pub(crate) fn new(
        chain_spec: Arc<ChainSpec>,
        miner: MiningMode,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        new_block_notification_sender: NewBlockNotificationSink,
        storage: Storage,
        client: Client,
        pool: Pool,
    ) -> Self {
        Self {
            chain_spec,
            client,
            miner,
            insert_task: None,
            storage,
            pool,
            to_engine,
            new_block_notification_sender,
            queued: Default::default(),
            pipe_line_events: None,
        }
    }

    /// Sets the pipeline events to listen on.
    pub fn set_pipeline_events(&mut self, events: UnboundedReceiverStream<PipelineEvent>) {
        self.pipe_line_events = Some(events);
    }
}

impl<Client, Pool> Future for MiningTask<Client, Pool>
where
    Client: StateProviderFactory + Clone + Unpin + 'static,
    Pool: TransactionPool + Unpin + 'static,
    <Pool as TransactionPool>::Transaction: IntoRecoveredTransaction,
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
                let mut events = this.pipe_line_events.take();
                let new_block_notification_sender = this.new_block_notification_sender.clone();

                // Create the mining future that creates a block, notifies the engine that drives
                // the pipeline
                this.insert_task = Some(Box::pin(async move {
                    let mut storage = storage.write().await;
                    let mut header = Header {
                        parent_hash: storage.best_hash,
                        ommers_hash: EMPTY_OMMER_ROOT,
                        beneficiary: Default::default(),
                        state_root: Default::default(),
                        transactions_root: Default::default(),
                        receipts_root: Default::default(),
                        withdrawals_root: None,
                        logs_bloom: Default::default(),
                        difficulty: U256::from(1),
                        number: storage.best_block + 1,
                        gas_limit: 30_000_000,
                        gas_used: 0,
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                        mix_hash: Default::default(),
                        nonce: 0,
                        base_fee_per_gas: None,
                        extra_data: Default::default(),
                    };

                    let transactions = transactions
                        .into_iter()
                        .map(|tx| tx.to_recovered_transaction().into_signed())
                        .collect::<Vec<_>>();

                    header.transactions_root = if transactions.is_empty() {
                        EMPTY_TRANSACTIONS
                    } else {
                        proofs::calculate_transaction_root(transactions.iter())
                    };

                    let block =
                        Block { header, body: transactions, ommers: vec![], withdrawals: None };

                    // execute the new block
                    let substate = SubState::new(State::new(client.latest().unwrap()));
                    let mut executor = Executor::new(chain_spec, substate);

                    trace!(target: "consensus::auto", transactions=?&block.body, "executing transactions");

                    match executor.execute_transactions(&block, U256::ZERO, None) {
                        Ok((res, gas_used)) => {
                            let Block { mut header, body, .. } = block;

                            // clear all transactions from pool
                            pool.remove_transactions(body.iter().map(|tx| tx.hash));

                            header.receipts_root = if res.receipts().is_empty() {
                                EMPTY_RECEIPTS
                            } else {
                                let receipts_with_bloom = res
                                    .receipts()
                                    .iter()
                                    .map(|r| r.clone().into())
                                    .collect::<Vec<ReceiptWithBloom>>();
                                proofs::calculate_receipt_root(receipts_with_bloom.iter())
                            };

                            let body =
                                BlockBody { transactions: body, ommers: vec![], withdrawals: None };
                            header.gas_used = gas_used;

                            storage.insert_new_block(header.clone(), body);

                            let new_hash = storage.best_hash;
                            let state = ForkchoiceState {
                                head_block_hash: new_hash,
                                finalized_block_hash: new_hash,
                                safe_block_hash: new_hash,
                            };
                            drop(storage);

                            // send the new update to the engine, this will trigger the pipeline to
                            // download the block, execute it and store it in the database.
                            let (tx, _rx) = oneshot::channel();
                            let _ = to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
                                state,
                                payload_attrs: None,
                                tx,
                            });
                            debug!(target: "consensus::auto", ?state, "sent fork choice update");

                            // wait for the pipeline to finish
                            if let Some(events) = events.as_mut() {
                                debug!(target: "consensus::auto", "waiting for finish stage event...");
                                // wait for the finish stage to
                                loop {
                                    if let Some(PipelineEvent::Running { stage_id, .. }) =
                                        events.next().await
                                    {
                                        if stage_id == FINISH {
                                            debug!(target: "consensus::auto", "received finish stage event");
                                            break
                                        }
                                    }
                                }
                            }

                            let header = header.seal_slow();
                            debug!(target: "consensus::auto", header=?header.hash(), "sending block notification");

                            // send block notification
                            let _ = new_block_notification_sender.send(Arc::new(header));
                        }
                        Err(err) => {
                            warn!(target: "consensus::auto", ?err, "failed to execute block")
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

impl<Client, Pool: TransactionPool> std::fmt::Debug for MiningTask<Client, Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiningTask").finish_non_exhaustive()
    }
}
