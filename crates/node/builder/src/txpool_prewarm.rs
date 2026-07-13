//! Transaction-pool backed candidate source for engine cache prewarming.

use alloy_primitives::B256;
use reth_engine_tree::tree::{
    TxPoolPrewarmSource, TxPoolPrewarmTransaction, TxPoolPrewarmTransactions,
};
use reth_primitives_traits::{NodePrimitives, TxTy};
use reth_transaction_pool::{
    BestTransactions, BestTransactionsAttributes, PoolTransaction, TransactionPool,
};
use std::{fmt::Debug, marker::PhantomData};

/// [`TransactionPool`]-backed [`TxPoolPrewarmSource`].
#[derive(Debug)]
pub(crate) struct PoolTxPoolPrewarmSource<N: NodePrimitives, P> {
    pool: P,
    _marker: PhantomData<N>,
}

impl<N: NodePrimitives, P: TransactionPool> PoolTxPoolPrewarmSource<N, P> {
    /// Creates a new txpool prewarm source.
    pub(crate) const fn new(pool: P) -> Self {
        Self { pool, _marker: PhantomData }
    }
}

impl<N, P> TxPoolPrewarmSource<N> for PoolTxPoolPrewarmSource<N, P>
where
    N: NodePrimitives,
    P: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<N>>>
        + Clone
        + Send
        + Sync
        + Debug
        + 'static,
{
    fn tracked_block_hash(&self) -> B256 {
        self.pool.block_info().last_seen_block_hash
    }

    fn best_transactions(&self, parent_hash: B256) -> Option<TxPoolPrewarmTransactions<N>> {
        let block_info = self.pool.block_info();
        if block_info.last_seen_block_hash != parent_hash {
            return None
        }

        let mut best =
            self.pool.best_transactions_with_attributes(BestTransactionsAttributes::new(
                block_info.pending_basefee,
                block_info.pending_blob_fee.map(|fee| u64::try_from(fee).unwrap_or(u64::MAX)),
            ));
        best.allow_updates_out_of_order();

        if self.pool.block_info().last_seen_block_hash != parent_hash {
            return None
        }

        Some(Box::new(best.map(|transaction| TxPoolPrewarmTransaction {
            hash: *transaction.hash(),
            sender: transaction.sender(),
            transaction: transaction.transaction.clone_into_consensus(),
        })))
    }
}
