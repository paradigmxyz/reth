use crate::{mode::MiningMode, Storage};
use futures_util::{future::BoxFuture, FutureExt};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{BlockBody, Header, IntoRecoveredTransaction};
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

/// A Future that listens for new ready transactions and puts new blocks into storage
pub struct MiningTask<Pool: TransactionPool> {
    /// The active miner
    miner: MiningMode,
    /// Single active future that inserts a new block into `storage`
    insert_task: Option<BoxFuture<'static, ()>>,
    /// Shared storage to insert new blocks
    storage: Storage,
    /// Pool where transactions are stored
    pool: Pool,
    /// backlog of sets of transactions ready to be mined
    queued: VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
    /// TODO: ideally this would just be a sender of hashes
    to_engine: UnboundedSender<BeaconEngineMessage>,
}

// === impl MiningTask ===

impl<Pool: TransactionPool> MiningTask<Pool> {
    /// Creates a new instance of the task
    pub(crate) fn new(
        miner: MiningMode,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        storage: Storage,
        pool: Pool,
    ) -> Self {
        Self { miner, insert_task: None, storage, pool, to_engine, queued: Default::default() }
    }
}

impl<Pool> Future for MiningTask<Pool>
where
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

            if let Some(mut fut) = this.insert_task.take() {
                match fut.poll_unpin(cx) {
                    Poll::Ready(_) => {
                        if this.queued.is_empty() {
                            break
                        }
                        // queue new insert task
                        let storage = this.storage.clone();
                        let transactions = this.queued.pop_front().expect("not empty");
                        let to_engine = this.to_engine.clone();
                        this.insert_task = Some(Box::pin(async move {
                            let mut storage = storage.write().await;
                            let header = Header {
                                parent_hash: storage.best_hash,
                                ommers_hash: Default::default(),
                                beneficiary: Default::default(),
                                state_root: Default::default(),
                                transactions_root: Default::default(),
                                receipts_root: Default::default(),
                                withdrawals_root: None,
                                logs_bloom: Default::default(),
                                difficulty: Default::default(),
                                number: storage.best_block + 1,
                                gas_limit: 30_0000,
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
                            let body = BlockBody {
                                transactions: transactions
                                    .into_iter()
                                    .map(|tx| tx.to_recovered_transaction().into_signed())
                                    .collect(),
                                ommers: vec![],
                                withdrawals: None,
                            };
                            storage.insert_new_block(header, body);

                            let new_hash = storage.best_hash;
                            let state = ForkchoiceState {
                                head_block_hash: new_hash,
                                finalized_block_hash: new_hash,
                                safe_block_hash: new_hash,
                            };
                            let (tx, _rx) = oneshot::channel();
                            let _ = to_engine
                                .send(BeaconEngineMessage::ForkchoiceUpdated(state, None, tx));
                        }));
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

impl<Pool: TransactionPool> std::fmt::Debug for MiningTask<Pool> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MiningTask").finish_non_exhaustive()
    }
}
