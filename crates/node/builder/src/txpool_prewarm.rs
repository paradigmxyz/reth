//! Transaction-pool backed candidate source for engine cache prewarming.

use alloy_primitives::B256;
use reth_engine_tree::tree::{
    TxPoolPrewarmSource as PrewarmSource, TxPoolPrewarmTransaction as Transaction,
    TxPoolPrewarmTransactions as Transactions,
};
use reth_primitives_traits::{NodePrimitives, TxTy};
use reth_transaction_pool::{
    BestTransactions, BestTransactionsAttributes, PoolTransaction, TransactionPool,
};
use std::fmt::Debug;

/// [`TransactionPool`]-backed [`PrewarmSource`].
#[derive(Debug)]
pub(crate) struct Source<P>(P);

impl<P> Source<P> {
    /// Creates a new txpool prewarm source.
    pub(crate) const fn new(pool: P) -> Self {
        Self(pool)
    }
}

impl<N, P> PrewarmSource<N> for Source<P>
where
    N: NodePrimitives,
    P: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<N>>>
        + Clone
        + Send
        + Sync
        + Debug
        + 'static,
{
    fn best_transactions(&self, parent_hash: B256) -> Option<Transactions<N>> {
        let block_info = self.0.block_info();
        if block_info.last_seen_block_hash != parent_hash {
            return None
        }

        let mut best =
            self.0.best_transactions_with_attributes(BestTransactionsAttributes::new(
                block_info.pending_basefee,
                block_info.pending_blob_fee.map(|fee| u64::try_from(fee).unwrap_or(u64::MAX)),
            ));
        best.allow_updates_out_of_order();
        best.skip_blobs();

        Some(Box::new(best.map(|transaction| Transaction {
            hash: *transaction.hash(),
            sender: transaction.sender(),
            transaction: transaction.transaction.clone_into_consensus(),
        })))
    }
}
