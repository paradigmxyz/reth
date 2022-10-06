//! Transaction Pool internals.
//!
//! Incoming transactions are validated first. The validation outcome can have 3 states:
//!
//!      1. Transaction can _never_ be valid
//!      2. Transaction is _currently_ valid
//!      3. Transaction is _currently_ invalid, but could potentially become valid in the future
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
//! Furthermore, the following characteristics fall under (3.):
//!
//!     a) Nonce of a transaction is higher than the expected nonce for the next transaction of its
//! sender. A distinction is made here whether multiple transactions from the same sender have
//! gapless nonce increments.         a)(1) If _no_ transaction is missing in a chain of multiple
//! transactions from the same sender (all nonce in row), all of them can in principle be executed
//! on the current state one after the other.         a)(2) If there's a nonce gap, then all
//! transactions after the missing transaction are blocked until the missing transaction arrives.
//!     b) Transaction does not meet the dynamic fee cap requirement introduced by EIP-1559: The fee
//! cap of the transaction needs to be no less than the base fee of block.
//!
//!
//! In essence the transaction pool is made of two separate sub-pools:
//!
//!      _Pending Pool_: Contains all transactions that are valid on the current state and satisfy
//! (3. a)(1): _No_ nonce gaps      _Queued Pool_: Contains all transactions that are currently
//! blocked by missing transactions: (3. a)(2): _With_ nonce gaps
//!
//! To account for the dynamic base fee requirement (3. b) which could render an EIP-1559 and all
//! subsequent transactions of the sender currently invalid, the pending pool itself consists of two
//! queues:
//!
//!      _Ready Queue_: Contains all transactions that can be executed on the current state
//!      _Parked Queue_: Contains all transactions that either do not currently meet the dynamic
//! base fee requirement or are blocked by a previous transaction that violates it.
//!
//! The classification of transaction in which queue it belongs depends on the current base fee and
//! must be updated after changes:
//!
//!      - Base Fee increases: recheck the _Ready Queue_ and evict transactions that don't satisfy
//!        the new base fee, or depend on a transaction that no longer satisfies it, and move them
//!        to the _Parked Queue_.
//!      - Base Fee decreases: recheck the _Parked Queue_ and move all transactions that now satisfy
//!        the new base fee to the _Ready Queue_.
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
//!     - _Pending_: pending transactions are transactions that fall under (2.). Those transactions
//!       are _currently_ ready to be executed and are stored in the `pending` sub-pool
//!     - _Queued_: queued transactions are transactions that fall under category (3.). Those
//!       transactions are _currently_ waiting for state changes that eventually move them into
//!       category (2.) and become pending.
use crate::{
    error::{PoolError, PoolResult},
    pool::{
        listener::PoolEventListener,
        pending::PendingTransactions,
        queued::{QueuedPoolTransaction, QueuedTransactions},
    },
    traits::PoolTransaction,
    validate::ValidPoolTransaction,
    BlockID, PoolClient, PoolConfig, TransactionOrdering, TransactionValidator, U256,
};
use fnv::FnvHashMap;
use futures::channel::mpsc::{channel, Receiver, Sender};
use parking_lot::{Mutex, RwLock};
use reth_primitives::{TxHash, U64};
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::Arc,
};
use tracing::{debug, trace, warn};

mod events;
mod listener;
mod pending;
mod queued;
mod transaction;

use crate::{
    identifier::{SenderId, TransactionId},
    validate::TransactionValidationOutcome,
};
pub use events::TransactionEvent;
pub use pending::TransactionsIterator;

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
    pool: RwLock<GraphPool<T>>,
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
            pool: RwLock::new(GraphPool::new(ordering)),
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
            TransactionValidationOutcome::Valid { balance, state_nonce, transaction } => {
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
    pub(crate) fn ready_transactions(&self) -> TransactionsIterator<T> {
        self.pool.read().ready_transactions()
    }
}

/// A pool that only manages transactions.
///
/// This pool maintains a dependency graph of transactions and provides the currently ready
/// transactions.
pub struct GraphPool<T: TransactionOrdering> {
    /// How to order transactions.
    ordering: Arc<T>,
    /// Contains the currently known info
    sender_info: FnvHashMap<SenderId, SenderInfo>,
    /// Sub-Pool of transactions that are ready and waiting to be executed
    pending: PendingTransactions<T>,
    /// Sub-Pool of transactions that are waiting for state changes that eventually turn them
    /// valid, so they can be moved in the `pending` pool.
    queued: QueuedTransactions<T>,
}

// === impl PoolInner ===

impl<T: TransactionOrdering> GraphPool<T> {
    /// Create a new graph pool instance.
    pub fn new(ordering: Arc<T>) -> Self {
        let pending = PendingTransactions::new(Arc::clone(&ordering));
        Self { ordering, sender_info: Default::default(), pending, queued: Default::default() }
    }

    /// Updates the pool based on the changed base fee.
    ///
    /// This enforces the dynamic fee requirement.
    /// If the `new_base_fee` is _higher_ than previous base fee, all EIP-1559 transactions in the
    /// ready queue that now violate the dynamic fee requirement need to parked.
    /// If the `new_base_fee` is _lower_ than the previous base fee, all parked transactions that
    /// now satisfy the dynamic fee requirement need to moved to the ready queue.
    pub(crate) fn update_base_fee(&mut self, new_base_fee: U256) {
        let _old_base_fee = self.pending.set_next_base_fee(new_base_fee);
        // TODO update according to the changed base_fee
        todo!()
    }

    /// Returns if the transaction for the given hash is already included in this pool
    pub(crate) fn contains(&self, tx_hash: &TxHash) -> bool {
        self.queued.contains(tx_hash) || self.pending.contains(tx_hash)
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block.
    pub(crate) fn ready_transactions(&self) -> TransactionsIterator<T> {
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
    /// With EIP-1559, transactions can become executable or not without any changes to the
    /// sender's balance or nonce and instead their feeCap determines whether the transaction is
    /// _currently_ (on the current state) ready or needs to be parked until the feeCap satisfies
    /// the block's baseFee.
    fn add_transaction(
        &mut self,
        tx: ValidPoolTransaction<T::Transaction>,
    ) -> PoolResult<AddedTransaction<T::Transaction>> {
        if self.contains(tx.hash()) {
            warn!(target: "txpool", "[{:?}] Already added", tx.hash());
            return Err(PoolError::AlreadyAdded(Box::new(*tx.hash())))
        }

        let tx = QueuedPoolTransaction::new(tx, self.pending.provided_dependencies());
        trace!(target: "txpool", "[{:?}] {:?}", tx.transaction.hash(), tx);

        // If all ids are not satisfied import to queued
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
        tx: QueuedPoolTransaction<T>,
    ) -> PoolResult<AddedTransaction<T::Transaction>> {
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
                .extend(self.queued.satisfy_and_unlock(&current_tx.transaction.transaction_id));

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

    /// Prunes the transactions that provide the given dependencies.
    ///
    /// This will effectively remove those transactions that satisfy the dependencies.
    /// And queued transactions might get promoted if the pruned dependencies unlock them.
    pub fn prune_transactions(
        &mut self,
        dependencies: impl IntoIterator<Item = TransactionId>,
    ) -> PruneResult<T::Transaction> {
        let mut imports = vec![];
        let mut pruned = vec![];

        for dependency in dependencies {
            // mark as satisfied and store the transactions that got unlocked
            imports.extend(self.queued.satisfy_and_unlock(&dependency));
            // prune transactions
            pruned.extend(self.pending.remove_mined(dependency));
        }

        let mut promoted = vec![];
        let mut failed = vec![];
        for tx in imports {
            let hash = *tx.transaction.hash();
            match self.add_pending_transaction(tx) {
                Ok(res) => promoted.push(res),
                Err(e) => {
                    warn!(target: "txpool", "Failed to promote tx [{:?}] : {:?}", hash, e);
                    failed.push(hash)
                }
            }
        }

        PruneResult { pruned, failed, promoted }
    }

    /// Remove the given transactions from the pool.
    pub fn remove_invalid(
        &mut self,
        tx_hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        // early exit in case there is no invalid transactions.
        if tx_hashes.is_empty() {
            return vec![]
        }
        trace!(target: "txpool", "Removing invalid transactions: {:?}", tx_hashes);

        let mut removed = self.pending.remove_with_dependencies(tx_hashes.clone(), None);
        removed.extend(self.queued.remove(tx_hashes));

        trace!(target: "txpool", "Removed invalid transactions: {:?}", removed);

        removed
    }

    /// Returns the current size of the entire pool
    pub fn size_of(&self) -> usize {
        unimplemented!()
    }

    /// Ensures that the transactions in the sub-pools are within the given bounds.
    ///
    /// If the current size exceeds the given bounds, the worst transactions are evicted from the
    /// pool and returned.
    pub fn enforce_size_limits(&mut self) {
        unimplemented!()
    }
}

/// Represents the outcome of a prune
pub struct PruneResult<T: PoolTransaction> {
    /// a list of added transactions that a pruned marker satisfied
    pub promoted: Vec<AddedTransaction<T>>,
    /// all transactions that  failed to be promoted and now are discarded
    pub failed: Vec<TxHash>,
    /// all transactions that were pruned from the ready pool
    pub pruned: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> fmt::Debug for PruneResult<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "PruneResult {{ ")?;
        write!(
            fmt,
            "promoted: {:?}, ",
            self.promoted.iter().map(|tx| *tx.hash()).collect::<Vec<_>>()
        )?;
        write!(fmt, "failed: {:?}, ", self.failed)?;
        write!(
            fmt,
            "pruned: {:?}, ",
            self.pruned.iter().map(|tx| *tx.transaction.hash()).collect::<Vec<_>>()
        )?;
        write!(fmt, "}}")?;
        Ok(())
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

/// Stores relevant context about a sender.
#[derive(Debug, Clone)]
struct SenderInfo {
    /// current nonce of the sender
    state_nonce: u64,
    /// Balance of the sender at the current point.
    balance: U256,
    /// How many transactions of this sender are currently in the pool.
    num_transactions: u64,
}

// === impl SenderInfo ===

impl SenderInfo {
    /// Creates a new entry for an incoming, not yet tracked sender.
    fn new_incoming(state_nonce: u64, balance: U256) -> Self {
        Self { state_nonce, balance, num_transactions: 1 }
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
