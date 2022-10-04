//! Transaction Pool internals.
//!
//! Incoming transactions are validated first. The validation outcome can have 3 states:
//!     1. Transaction can _never_ be valid
//!     2. Transaction is _currently_ valid
//!     3. Transaction is _currently_ invalid, but could potentially become valid in the future
//!
//! However, (2.) and (3.) of a transaction can only be determined on the basis of the current
//! state, whereas (1.) holds indefinitely. This means once the state changes (2.) and (3.) need to
//! be reevaluated again.
//!
//! The transaction pool is responsible for storing new, valid transactions and providing the next
//! best transactions sorted by their priority. Where priority is determined by the transaction's
//! score.
//!
//! However, the score is also only valid for the current state.
//!
//! In essence the transaction pool is made of two separate sub-pools for currently valid (2.) and
//! currently invalid (3.).
//!
//! Depending on the use case, consumers of the [`TransactionPool`](crate::traits::TransactionPool)
//! are interested in (2.) and/or (3.).

//! A generic [`TransactionPool`](crate::traits::TransactionPool) that only handles transactions.
//!
//! This Pool maintains two separate sub-pools for (2.) and (3.)
//!
//! ## Terminology
//!
//!     - _Pending_: pending transactions are transactions that fall under (2.). Those transactions
//!       are _currently_ ready to be executed and are stored in the `pending` sub-pool
//!     - _Queued_: queued transactions are transactions that fall under category (3.). Those
//!       transactions are _currently_ waiting for state changes that eventually move them into
//!       category (2.) and become pending.
use crate::{
    error::{PoolError, PoolResult},
    pool::{
        listener::PoolEventListener,
        pending::{PendingTransactions, TransactionsIterator},
        queued::{QueuedPoolTransaction, QueuedTransactions},
    },
    traits::PoolTransaction,
    validate::ValidPoolTransaction,
    PoolClient, PoolConfig, TransactionOrdering, TransactionValidator,
};
use parking_lot::RwLock;
use reth_primitives::TxHash;
use std::{collections::VecDeque, fmt, sync::Arc};
use tracing::{debug, trace, warn};

mod events;
mod listener;
mod pending;
mod queued;
mod transaction;

// TODO find better name
pub struct Pool<PoolApi: PoolClient, Ordering: TransactionOrdering> {
    pool: Arc<PoolInner<PoolApi, Ordering>>,
}

type TransactionHashFor<PoolApi> =
    <<PoolApi as TransactionValidator>::Transaction as PoolTransaction>::Hash;

// A pool that manages transactions
pub struct PoolInner<PoolApi: PoolClient, Ordering: TransactionOrdering> {
    /// Chain/Storage access.
    client: Arc<PoolApi>,
    /// How to order transactions.
    ordering: Arc<Ordering>,
    /// Pool settings.
    config: PoolConfig,
    /// Listeners for transaction state change events.
    listeners: RwLock<PoolEventListener<TransactionHashFor<PoolApi>, PoolApi::BlockHash>>,
    /// Sub-Pool of transactions that are ready and waiting to be executed
    pending: PendingTransactions<PoolApi::Transaction, Ordering>,
    /// Sub-Pool of transactions that are waiting for state changes that eventually turn them
    /// valid, so they can be moved in the `pending` pool.
    queued: QueuedTransactions<PoolApi::Transaction>,
}

// === impl PoolInner ===

impl<PoolApi: PoolClient, O: TransactionOrdering> PoolInner<PoolApi, O> {
    /// Returns if the transaction for the given hash is already included in this pool
    pub fn contains(&self, tx_hash: &TransactionHashFor<PoolApi>) -> bool {
        self.queued.contains(tx_hash) || self.pending.contains(tx_hash)
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block.
    pub fn ready(&self) -> TransactionsIterator<PoolApi::Transaction, O> {
        self.pending.get_transactions()
    }

    /// Adds the transaction into the pool
    ///
    /// This pool consists of two sub-pools: `Queued` and `Pending`.
    ///
    /// The `Queued` pool contains transaction with gaps in its dependency tree: It requires
    /// additional transaction that are note yet present in the pool.
    ///
    /// The `Pending` pool contains all transactions that have all their dependencies satisfied (no
    /// nonce gaps). It consists of two parts: `Parked` and `Ready`.
    ///
    /// The `Ready` queue contains transactions that are ready to be included in the pending block.
    /// With the EIP-1559, transactions can become executable or not without any changes to the
    /// sender's balance or nonce and instead their feeCap determines whether the transaction is
    /// _currently_ (on the current state) ready or needs to be parked until the feeCap satisfies
    /// the block's basFee.
    fn add_transaction(
        &mut self,
        tx: ValidPoolTransaction<PoolApi::Transaction>,
    ) -> PoolResult<AddedTransaction<PoolApi::Transaction>> {
        if self.contains(tx.hash()) {
            warn!(target: "txpool", "[{:?}] Already added", tx.hash());
            return Err(PoolError::AlreadyAdded(Box::new(*tx.hash())))
        }

        let tx = QueuedPoolTransaction::new(tx, self.pending.provided_dependencies());
        trace!(target: "txpool", "[{:?}] {:?}", tx.transaction.hash(), tx);

        // If all markers are not satisfied import to future
        if !tx.is_satisfied() {
            let hash = *tx.transaction.hash();
            self.queued.add_transaction(tx)?;
            return Ok(AddedTransaction::Queued { hash })
        }
        self.add_pending_transaction(tx)
    }

    /// Adds the transaction to the pending pool.
    ///
    /// This will also move all transaction that get unlocked by the dependency id this transaction
    /// provides from the queued pool into the pending pool.
    ///
    /// CAUTION: this expects that transaction's dependencies are fully satisfied
    fn add_pending_transaction(
        &mut self,
        tx: QueuedPoolTransaction<PoolApi::Transaction>,
    ) -> PoolResult<AddedTransaction<PoolApi::Transaction>> {
        let hash = *tx.transaction.hash();
        trace!(target: "txpool", "adding pending transaction [{:?}]", hash);
        let mut pending = AddedPendingTransaction::new(hash);

        // tracks all transaction that can be moved to the pending pool, starting the given
        // transaction
        let mut pending_transactions = VecDeque::from([tx]);
        // tracks whether we're processing the given `tx`
        let mut is_new_tx = true;

        // take first transaction from the list
        while let Some(current_tx) = pending_transactions.pop_front() {
            // also add the transaction that the current transaction unlocks
            pending_transactions
                .extend(self.queued.satisfy_and_unlock(&current_tx.transaction.provides));

            let current_hash = *current_tx.transaction.hash();

            // try to add the transaction to the ready pool
            match self.pending.add_transaction(current_tx) {
                Ok(replaced_transactions) => {
                    if !is_new_tx {
                        pending.promoted.push(current_hash);
                    }
                    // tx removed from ready pool
                    pending.removed.extend(replaced_transactions);
                }
                Err(err) => {
                    // failed to add transaction
                    if is_new_tx {
                        debug!(target: "txpool", "[{:?}] Failed to add tx: {:?}", current_hash,
        err);
                        return Err(err)
                    } else {
                        pending.discarded.push(current_hash);
                    }
                }
            }
            is_new_tx = false;
        }

        // check for a cycle where importing a transaction resulted in pending transactions to be
        // added while removing current transaction. in which case we move this transaction back to
        // the pending queue
        if pending.removed.iter().any(|tx| *tx.hash() == hash) {
            self.pending.clear_transactions(&pending.promoted);
            return Err(PoolError::CyclicTransaction)
        }

        Ok(AddedTransaction::Pending(pending))
    }
}

// /// Represents the outcome of a prune
// pub struct PruneResult {
//     /// a list of added transactions that a pruned marker satisfied
//     pub promoted: Vec<AddedTransaction>,
//     /// all transactions that  failed to be promoted and now are discarded
//     pub failed: Vec<TxHash>,
//     /// all transactions that were pruned from the ready pool
//     pub pruned: Vec<Arc<PoolTransaction>>,
// }
//
// impl fmt::Debug for PruneResult {
//     fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
//         write!(fmt, "PruneResult {{ ")?;
//         write!(
//             fmt,
//             "promoted: {:?}, ",
//             self.promoted.iter().map(|tx| *tx.hash()).collect::<Vec<_>>()
//         )?;
//         write!(fmt, "failed: {:?}, ", self.failed)?;
//         write!(
//             fmt,
//             "pruned: {:?}, ",
//             self.pruned.iter().map(|tx| *tx.pending_transaction.hash()).collect::<Vec<_>>()
//         )?;
//         write!(fmt, "}}")?;
//         Ok(())
//     }
// }

#[derive(Debug, Clone)]
pub struct AddedPendingTransaction<T: PoolTransaction> {
    /// the hash of the submitted transaction
    hash: T::Hash,
    /// transactions promoted to the ready queue
    promoted: Vec<T::Hash>,
    /// transaction that failed and became discarded
    discarded: Vec<T::Hash>,
    /// Transactions removed from the Ready pool
    removed: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> AddedPendingTransaction<T> {
    pub fn new(hash: T::Hash) -> Self {
        Self {
            hash,
            promoted: Default::default(),
            discarded: Default::default(),
            removed: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum AddedTransaction<T: PoolTransaction> {
    /// Transaction was successfully added and moved to the pending pool.
    Pending(AddedPendingTransaction<T>),
    /// Transaction was successfully added but not yet queued for processing and moved to the
    /// queued pool instead.
    Queued {
        /// the hash of the submitted transaction
        hash: T::Hash,
    },
}

impl<T: PoolTransaction> AddedTransaction<T> {
    pub fn hash(&self) -> &T::Hash {
        match self {
            AddedTransaction::Pending(tx) => &tx.hash,
            AddedTransaction::Queued { hash } => hash,
        }
    }
}
