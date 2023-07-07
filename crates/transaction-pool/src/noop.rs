//! A transaction pool implementation that does nothing.
//!
//! This is useful for wiring components together that don't require an actual pool but still need
//! to be generic over it.

use crate::{
    error::PoolError, AllPoolTransactions, AllTransactionsEvents, BestTransactions, BlockInfo,
    NewTransactionEvent, PoolResult, PoolSize, PoolTransaction, PooledTransaction,
    PropagatedTransactions, TransactionEvents, TransactionOrigin, TransactionPool,
    TransactionValidationOutcome, TransactionValidator, ValidPoolTransaction,
};
use reth_primitives::{Address, TxHash};
use std::{marker::PhantomData, sync::Arc};
use tokio::sync::{mpsc, mpsc::Receiver};

/// A [`TransactionPool`] implementation that does nothing.
///
/// All transactions are rejected and no events are emitted.
/// This type will never hold any transactions and is only useful for wiring components together.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NoopTransactionPool;

#[async_trait::async_trait]
impl TransactionPool for NoopTransactionPool {
    type Transaction = PooledTransaction;

    fn pool_size(&self) -> PoolSize {
        Default::default()
    }

    fn block_info(&self) -> BlockInfo {
        BlockInfo {
            last_seen_block_hash: Default::default(),
            last_seen_block_number: 0,
            pending_basefee: 0,
        }
    }

    async fn add_transaction_and_subscribe(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TransactionEvents> {
        let hash = *transaction.hash();
        Err(PoolError::Other(hash, Box::new(NoopInsertError::new(transaction))))
    }

    async fn add_transaction(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> PoolResult<TxHash> {
        let hash = *transaction.hash();
        Err(PoolError::Other(hash, Box::new(NoopInsertError::new(transaction))))
    }

    async fn add_transactions(
        &self,
        _origin: TransactionOrigin,
        transactions: Vec<Self::Transaction>,
    ) -> PoolResult<Vec<PoolResult<TxHash>>> {
        Ok(transactions
            .into_iter()
            .map(|transaction| {
                let hash = *transaction.hash();
                Err(PoolError::Other(hash, Box::new(NoopInsertError::new(transaction))))
            })
            .collect())
    }

    fn transaction_event_listener(&self, _tx_hash: TxHash) -> Option<TransactionEvents> {
        None
    }

    fn all_transactions_event_listener(&self) -> AllTransactionsEvents<Self::Transaction> {
        AllTransactionsEvents { events: mpsc::channel(1).1 }
    }

    fn pending_transactions_listener(&self) -> Receiver<TxHash> {
        mpsc::channel(1).1
    }

    fn new_transactions_listener(&self) -> Receiver<NewTransactionEvent<Self::Transaction>> {
        mpsc::channel(1).1
    }

    fn pooled_transaction_hashes(&self) -> Vec<TxHash> {
        vec![]
    }

    fn pooled_transaction_hashes_max(&self, _max: usize) -> Vec<TxHash> {
        vec![]
    }

    fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn pooled_transactions_max(
        &self,
        _max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn best_transactions(
        &self,
    ) -> Box<dyn BestTransactions<Item = Arc<ValidPoolTransaction<Self::Transaction>>>> {
        Box::new(std::iter::empty())
    }

    fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn all_transactions(&self) -> AllPoolTransactions<Self::Transaction> {
        AllPoolTransactions::default()
    }

    fn remove_transactions(
        &self,
        _hashes: impl IntoIterator<Item = TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn retain_unknown(&self, _hashes: &mut Vec<TxHash>) {}

    fn get(&self, _tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<Self::Transaction>>> {
        None
    }

    fn get_all(
        &self,
        _txs: impl IntoIterator<Item = TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }

    fn on_propagated(&self, _txs: PropagatedTransactions) {}

    fn get_transactions_by_sender(
        &self,
        _sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<Self::Transaction>>> {
        vec![]
    }
}

/// A [`TransactionValidator`] that does nothing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NoopTransactionValidator<T>(PhantomData<T>);

#[async_trait::async_trait]
impl<T: PoolTransaction> TransactionValidator for NoopTransactionValidator<T> {
    type Transaction = T;

    async fn validate_transaction(
        &self,
        _origin: TransactionOrigin,
        transaction: Self::Transaction,
    ) -> TransactionValidationOutcome<Self::Transaction> {
        TransactionValidationOutcome::Valid {
            balance: Default::default(),
            state_nonce: 0,
            transaction,
            propagate: true,
        }
    }
}

impl<T> Default for NoopTransactionValidator<T> {
    fn default() -> Self {
        NoopTransactionValidator(PhantomData)
    }
}

/// An error that contains the transaction that failed to be inserted into the noop pool.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Can't insert transaction into the noop pool that does nothing.")]
pub struct NoopInsertError {
    tx: PooledTransaction,
}

impl NoopInsertError {
    fn new(tx: PooledTransaction) -> Self {
        Self { tx }
    }

    /// Returns the transaction that failed to be inserted.
    pub fn into_inner(self) -> PooledTransaction {
        self.tx
    }
}
