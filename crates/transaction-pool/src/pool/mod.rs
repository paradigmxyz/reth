//! Transaction Pool internals.
//!
//! Incoming transactions are before they enter the pool first. The validation outcome can have 3
//! states:
//!
//!  1. Transaction can _never_ be valid
//!  2. Transaction is _currently_ valid
//!  3. Transaction is _currently_ invalid, but could potentially become valid in the future
//!
//! However, (2.) and (3.) of a transaction can only be determined on the basis of the current
//! state, whereas (1.) holds indefinitely. This means once the state changes (2.) and (3.) the
//! state of a transaction needs to be reevaluated again.
//!
//! The transaction pool is responsible for storing new, valid transactions and providing the next
//! best transactions sorted by their priority. Where priority is determined by the transaction's
//! score ([`TransactionOrdering`]).
//!
//! Furthermore, the following characteristics fall under (3.):
//!
//!  a) Nonce of a transaction is higher than the expected nonce for the next transaction of its
//! sender. A distinction is made here whether multiple transactions from the same sender have
//! gapless nonce increments.
//!
//!  a)(1) If _no_ transaction is missing in a chain of multiple
//! transactions from the same sender (all nonce in row), all of them can in principle be executed
//! on the current state one after the other.
//!
//!  a)(2) If there's a nonce gap, then all
//! transactions after the missing transaction are blocked until the missing transaction arrives.
//!
//!  b) Transaction does not meet the dynamic fee cap requirement introduced by EIP-1559: The
//! fee cap of the transaction needs to be no less than the base fee of block.
//!
//!
//! In essence the transaction pool is made of three separate sub-pools:
//!
//!  - Pending Pool: Contains all transactions that are valid on the current state and satisfy
//! (3. a)(1): _No_ nonce gaps
//!
//!  - Queued Pool: Contains all transactions that are currently blocked by missing
//! transactions: (3. a)(2): _With_ nonce gaps or due to lack of funds.
//!
//!  - Basefee Pool: To account for the dynamic base fee requirement (3. b) which could render
//! an EIP-1559 and all subsequent transactions of the sender currently invalid.
//!
//! The classification of transactions is always dependent on the current state that is changed as
//! soon as a new block is mined. Once a new block is mined, the account changeset must be applied
//! to the transaction pool.
//!
//!
//! Depending on the use case, consumers of the [`TransactionPool`](crate::traits::TransactionPool)
//! are interested in (2.) and/or (3.).

//! A generic [`TransactionPool`](crate::traits::TransactionPool) that only handles transactions.
//!
//! This Pool maintains two separate sub-pools for (2.) and (3.)
//!
//! ## Terminology
//!
//!  - _Pending_: pending transactions are transactions that fall under (2.). Those transactions are
//!    _currently_ ready to be executed and are stored in the pending sub-pool
//!  - _Queued_: queued transactions are transactions that fall under category (3.). Those
//!    transactions are _currently_ waiting for state changes that eventually move them into
//!    category (2.) and become pending.

use crate::{
    error::PoolResult, pool::listener::PoolEventListener, traits::PoolTransaction,
    validate::ValidPoolTransaction, BlockID, PoolClient, PoolConfig, TransactionOrdering,
    TransactionValidator, U256,
};

use best::BestTransactions;
use futures::channel::mpsc::{channel, Receiver, Sender};
use parking_lot::{Mutex, RwLock};
use reth_primitives::{TxHash, U64};
use std::{collections::HashMap, sync::Arc};
use tracing::warn;

mod best;
mod events;
mod listener;
mod parked;
mod pending;
pub(crate) mod state;
mod transaction;
pub mod txpool;

use crate::{pool::txpool::TxPool, validate::TransactionValidationOutcome};
pub use events::TransactionEvent;

/// Shareable Transaction pool.
pub struct BasicPool<P: PoolClient, T: TransactionOrdering> {
    /// Arc'ed instance of the pool internals
    pool: Arc<PoolInner<P, T>>,
}

// === impl Pool ===

impl<P: PoolClient, T: TransactionOrdering> BasicPool<P, T>
where
    P: PoolClient,
    T: TransactionOrdering<Transaction = <P as TransactionValidator>::Transaction>,
{
    /// Create a new transaction pool instance.
    pub fn new(client: Arc<P>, ordering: Arc<T>, config: PoolConfig) -> Self {
        Self { pool: Arc::new(PoolInner::new(client, ordering, config)) }
    }

    /// Returns the wrapped pool
    pub(crate) fn inner(&self) -> &PoolInner<P, T> {
        &self.pool
    }

    /// Returns the actual block number for the block id
    fn resolve_block_number(&self, block_id: &BlockID) -> PoolResult<U64> {
        self.pool.client().ensure_block_number(block_id)
    }

    /// Add a single _unverified_ transaction into the pool.
    pub async fn add_transaction(
        &self,
        block_id: &BlockID,
        transaction: P::Transaction,
    ) -> PoolResult<TxHash> {
        self.add_transactions(block_id, Some(transaction))
            .await?
            .pop()
            .expect("transaction exists; qed")
    }

    /// Adds all given transactions into the pool
    pub async fn add_transactions(
        &self,
        block_id: &BlockID,
        transactions: impl IntoIterator<Item = P::Transaction>,
    ) -> PoolResult<Vec<PoolResult<TxHash>>> {
        let validated = self.validate_all(block_id, transactions).await?;
        let transactions = self.pool.add_transactions(validated.into_values());
        Ok(transactions)
    }

    /// Returns future that validates all transaction in the given iterator at the block the
    /// `block_id` points to.
    async fn validate_all(
        &self,
        block_id: &BlockID,
        transactions: impl IntoIterator<Item = P::Transaction>,
    ) -> PoolResult<HashMap<TxHash, TransactionValidationOutcome<P::Transaction>>> {
        // get the actual block number which is required to validate the transactions
        let block_number = self.resolve_block_number(block_id)?;

        let outcome = futures::future::join_all(
            transactions.into_iter().map(|tx| self.validate(block_id, block_number, tx)),
        )
        .await
        .into_iter()
        .collect::<HashMap<_, _>>();

        Ok(outcome)
    }

    /// Validates the given transaction at the given block
    async fn validate(
        &self,
        block_id: &BlockID,
        _block_number: U64,
        transaction: P::Transaction,
    ) -> (TxHash, TransactionValidationOutcome<P::Transaction>) {
        let _hash = *transaction.hash();
        // TODO this is where additional validate checks would go, like banned senders etc...
        let _res = self.pool.client().validate_transaction(block_id, transaction).await;

        // TODO blockstamp the transaction

        todo!()
    }

    /// Registers a new transaction listener and returns the receiver stream.
    pub fn ready_transactions_listener(&self) -> Receiver<TxHash> {
        self.pool.add_ready_listener()
    }
}

impl<P: PoolClient, O: TransactionOrdering> Clone for BasicPool<P, O> {
    fn clone(&self) -> Self {
        Self { pool: Arc::clone(&self.pool) }
    }
}

/// Transaction pool internals.
pub struct PoolInner<P: PoolClient, T: TransactionOrdering> {
    /// Chain/Storage access.
    client: Arc<P>,
    /// The internal pool that manages
    pool: RwLock<TxPool<T>>,
    /// Pool settings.
    config: PoolConfig,
    /// Manages listeners for transaction state change events.
    event_listener: RwLock<PoolEventListener<TxHash>>,
    /// Listeners for new ready transactions.
    ready_transaction_listener: Mutex<Vec<Sender<TxHash>>>,
}

// === impl PoolInner ===

impl<P: PoolClient, T: TransactionOrdering> PoolInner<P, T>
where
    P: PoolClient,
    T: TransactionOrdering<Transaction = <P as TransactionValidator>::Transaction>,
{
    /// Create a new transaction pool instance.
    pub fn new(client: Arc<P>, ordering: Arc<T>, config: PoolConfig) -> Self {
        Self {
            client,
            config,
            event_listener: Default::default(),
            pool: RwLock::new(TxPool::new(ordering)),
            ready_transaction_listener: Default::default(),
        }
    }

    /// Updates the pool
    pub(crate) fn update_base_fee(&self, base_fee: U256) {
        self.pool.write().update_base_fee(base_fee);
    }

    /// Get client reference.
    pub fn client(&self) -> &P {
        &self.client
    }

    /// Adds a new transaction listener to the pool that gets notified about every new ready
    /// transaction
    pub fn add_ready_listener(&self) -> Receiver<TxHash> {
        const TX_LISTENER_BUFFER_SIZE: usize = 2048;
        let (tx, rx) = channel(TX_LISTENER_BUFFER_SIZE);
        self.ready_transaction_listener.lock().push(tx);
        rx
    }

    /// Resubmits transactions back into the pool.
    pub fn resubmit(&self, _transactions: HashMap<TxHash, ValidPoolTransaction<T::Transaction>>) {
        unimplemented!()
    }

    /// Add a single validated transaction into the pool.
    fn add_transaction(
        &self,
        tx: TransactionValidationOutcome<T::Transaction>,
    ) -> PoolResult<TxHash> {
        match tx {
            TransactionValidationOutcome::Valid { balance: _, state_nonce: _, transaction: _ } => {
                // TODO create `ValidPoolTransaction`

                // let added = self.pool.write().add_transaction(tx)?;
                //
                // if let Some(ready) = added.as_ready() {
                //     self.on_new_ready_transaction(ready);
                // }
                //
                // self.notify_event_listeners(&added);
                //
                // Ok(*added.hash())

                todo!()
            }
            TransactionValidationOutcome::Invalid(_tx, err) => {
                // TODO notify listeners about invalid
                Err(err)
            }
        }
    }

    /// Adds all transactions in the iterator to the pool, returning a list of results.
    pub fn add_transactions(
        &self,
        transactions: impl IntoIterator<Item = TransactionValidationOutcome<T::Transaction>>,
    ) -> Vec<PoolResult<TxHash>> {
        // TODO check pool limits

        transactions.into_iter().map(|tx| self.add_transaction(tx)).collect::<Vec<_>>()
    }

    /// Notify all listeners about the new transaction.
    fn on_new_ready_transaction(&self, ready: &TxHash) {
        let mut transaction_listeners = self.ready_transaction_listener.lock();
        transaction_listeners.retain_mut(|listener| match listener.try_send(*ready) {
            Ok(()) => true,
            Err(e) => {
                if e.is_full() {
                    warn!(
                        target: "txpool",
                        "[{:?}] dropping full ready transaction listener",
                        ready,
                    );
                    true
                } else {
                    false
                }
            }
        });
    }

    /// Fire events for the newly added transaction.
    fn notify_event_listeners(&self, tx: &AddedTransaction<T::Transaction>) {
        let mut listener = self.event_listener.write();

        match tx {
            AddedTransaction::Pending(tx) => {
                listener.ready(&tx.hash, None);
                // TODO  more listeners for discarded, removed etc...
            }
            AddedTransaction::Queued { hash } => {
                listener.queued(hash);
            }
        }
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block.
    pub(crate) fn ready_transactions(&self) -> BestTransactions<T> {
        self.pool.read().best_transactions()
    }
}

/// Tracks an added transaction and all graph changes caused by adding it.
#[derive(Debug, Clone)]
pub struct AddedPendingTransaction<T: PoolTransaction> {
    /// the hash of the submitted transaction
    hash: TxHash,
    /// transactions promoted to the ready queue
    promoted: Vec<TxHash>,
    /// transaction that failed and became discarded
    discarded: Vec<TxHash>,
    /// Transactions removed from the Ready pool
    removed: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> AddedPendingTransaction<T> {
    /// Create a new, empty transaction.
    fn new(hash: TxHash) -> Self {
        Self {
            hash,
            promoted: Default::default(),
            discarded: Default::default(),
            removed: Default::default(),
        }
    }
}

/// Represents a transaction that was added into the pool and its state
#[derive(Debug, Clone)]
pub enum AddedTransaction<T: PoolTransaction> {
    /// Transaction was successfully added and moved to the pending pool.
    Pending(AddedPendingTransaction<T>),
    /// Transaction was successfully added but not yet queued for processing and moved to the
    /// queued pool instead.
    Queued {
        /// the hash of the submitted transaction
        hash: TxHash,
    },
}

impl<T: PoolTransaction> AddedTransaction<T> {
    /// Returns the hash of the transaction if it's ready
    pub fn as_ready(&self) -> Option<&TxHash> {
        if let AddedTransaction::Pending(tx) = self {
            Some(&tx.hash)
        } else {
            None
        }
    }

    /// Returns the hash of the transaction
    pub fn hash(&self) -> &TxHash {
        match self {
            AddedTransaction::Pending(tx) => &tx.hash,
            AddedTransaction::Queued { hash } => hash,
        }
    }
}
