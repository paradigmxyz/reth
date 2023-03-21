use crate::{mode::MiningMode, Storage};
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

/// A
pub(crate) struct MiningTask<Pool: TransactionPool> {
    /// The active miner
    miner: MiningMode,

    /// Shared storage to insert new blocks
    storage: Storage,
    /// Pool where transactions are stored
    pool: Pool,
    /// backlog of sets of transactions ready to be mined
    queued: VecDeque<Vec<Arc<ValidPoolTransaction<<Pool as TransactionPool>::Transaction>>>>,
}

// === impl MiningTask ===

impl<Pool: TransactionPool> MiningTask<Pool> {
    /// Creates a new instance of the task
    pub(crate) fn new(miner: MiningMode, storage: Storage, pool: Pool) -> Self {
        Self { miner, storage, pool, queued: Default::default() }
    }
}

impl<Pool> Future for MiningTask<Pool>
where
    Pool: TransactionPool + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // this drives block production and
        loop {
            if let Poll::Ready(transactions) = this.miner.poll(&this.pool, cx) {
                // miner returned a set of transaction that we feed to the producer
                this.queued.push_back(transactions);
            } else {
                // no progress made
                break
            }
        }

        todo!()
    }
}
