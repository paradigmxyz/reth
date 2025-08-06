//! Transaction Pool internals.
//!
//! Incoming transactions are validated before they enter the pool first. The validation outcome can
//! have 3 states:
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
//!  - Pending Pool: Contains all transactions that are valid on the current state and satisfy (3.
//!    a)(1): _No_ nonce gaps. A _pending_ transaction is considered _ready_ when it has the lowest
//!    nonce of all transactions from the same sender. Once a _ready_ transaction with nonce `n` has
//!    been executed, the next highest transaction from the same sender `n + 1` becomes ready.
//!
//!  - Queued Pool: Contains all transactions that are currently blocked by missing transactions:
//!    (3. a)(2): _With_ nonce gaps or due to lack of funds.
//!
//!  - Basefee Pool: To account for the dynamic base fee requirement (3. b) which could render an
//!    EIP-1559 and all subsequent transactions of the sender currently invalid.
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
//!  - _Pending_: pending transactions are transactions that fall under (2.). These transactions can
//!    currently be executed and are stored in the pending sub-pool
//!  - _Queued_: queued transactions are transactions that fall under category (3.). Those
//!    transactions are _currently_ waiting for state changes that eventually move them into
//!    category (2.) and become pending.

use crate::{
    blobstore::BlobStore,
    error::{PoolError, PoolErrorKind, PoolResult},
    identifier::{SenderId, SenderIdentifiers, TransactionId},
    metrics::BlobStoreMetrics,
    pool::{
        listener::{
            BlobTransactionSidecarListener, PendingTransactionHashListener, PoolEventBroadcast,
            TransactionListener,
        },
        state::SubPool,
        txpool::{SenderInfo, TxPool},
        update::UpdateOutcome,
    },
    traits::{
        AllPoolTransactions, BestTransactionsAttributes, BlockInfo, GetPooledTransactionLimit,
        NewBlobSidecar, PoolSize, PoolTransaction, PropagatedTransactions, TransactionOrigin,
    },
    validate::{TransactionValidationOutcome, ValidPoolTransaction, ValidTransaction},
    CanonicalStateUpdate, EthPoolTransaction, PoolConfig, TransactionOrdering,
    TransactionValidator,
};

use alloy_primitives::{Address, TxHash, B256};
use best::BestTransactions;
use parking_lot::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use reth_eth_wire_types::HandleMempoolData;
use reth_execution_types::ChangedAccount;

use alloy_eips::{eip7594::BlobTransactionSidecarVariant, Typed2718};
use reth_primitives_traits::Recovered;
use rustc_hash::FxHashMap;
use std::{collections::HashSet, fmt, sync::Arc, time::Instant};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
mod events;
pub use best::{BestTransactionFilter, BestTransactionsWithPrioritizedSenders};
pub use blob::{blob_tx_priority, fee_delta, BlobOrd, BlobTransactions};
pub use events::{FullTransactionEvent, NewTransactionEvent, TransactionEvent};
pub use listener::{AllTransactionsEvents, TransactionEvents, TransactionListenerKind};
pub use parked::{BasefeeOrd, ParkedOrd, ParkedPool, QueuedOrd};
pub use pending::PendingPool;
use reth_primitives_traits::Block;

mod best;
mod blob;
mod listener;
mod parked;
pub(crate) mod pending;
pub(crate) mod size;
pub(crate) mod state;
pub mod txpool;
mod update;

/// Bound on number of pending transactions from `reth_network::TransactionsManager` to buffer.
pub const PENDING_TX_LISTENER_BUFFER_SIZE: usize = 2048;
/// Bound on number of new transactions from `reth_network::TransactionsManager` to buffer.
pub const NEW_TX_LISTENER_BUFFER_SIZE: usize = 1024;

const BLOB_SIDECAR_LISTENER_BUFFER_SIZE: usize = 512;

/// Transaction pool internals.
pub struct PoolInner<V, T, S>
where
    T: TransactionOrdering,
{
    /// Internal mapping of addresses to plain ints.
    identifiers: RwLock<SenderIdentifiers>,
    /// Transaction validator.
    validator: V,
    /// Storage for blob transactions
    blob_store: S,
    /// The internal pool that manages all transactions.
    pool: RwLock<TxPool<T>>,
    /// Pool settings.
    config: PoolConfig,
    /// Manages listeners for transaction state change events.
    event_listener: RwLock<PoolEventBroadcast<T::Transaction>>,
    /// Listeners for new _full_ pending transactions.
    pending_transaction_listener: Mutex<Vec<PendingTransactionHashListener>>,
    /// Listeners for new transactions added to the pool.
    transaction_listener: Mutex<Vec<TransactionListener<T::Transaction>>>,
    /// Listener for new blob transaction sidecars added to the pool.
    blob_transaction_sidecar_listener: Mutex<Vec<BlobTransactionSidecarListener>>,
    /// Metrics for the blob store
    blob_store_metrics: BlobStoreMetrics,
}

// === impl PoolInner ===

impl<V, T, S> PoolInner<V, T, S>
where
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    S: BlobStore,
{
    /// Create a new transaction pool instance.
    pub fn new(validator: V, ordering: T, blob_store: S, config: PoolConfig) -> Self {
        Self {
            identifiers: Default::default(),
            validator,
            event_listener: Default::default(),
            pool: RwLock::new(TxPool::new(ordering, config.clone())),
            pending_transaction_listener: Default::default(),
            transaction_listener: Default::default(),
            blob_transaction_sidecar_listener: Default::default(),
            config,
            blob_store,
            blob_store_metrics: Default::default(),
        }
    }

    /// Returns the configured blob store.
    pub const fn blob_store(&self) -> &S {
        &self.blob_store
    }

    /// Returns stats about the size of the pool.
    pub fn size(&self) -> PoolSize {
        self.get_pool_data().size()
    }

    /// Returns the currently tracked block
    pub fn block_info(&self) -> BlockInfo {
        self.get_pool_data().block_info()
    }
    /// Sets the currently tracked block
    pub fn set_block_info(&self, info: BlockInfo) {
        self.pool.write().set_block_info(info)
    }

    /// Returns the internal [`SenderId`] for this address
    pub fn get_sender_id(&self, addr: Address) -> SenderId {
        self.identifiers.write().sender_id_or_create(addr)
    }

    /// Returns the internal [`SenderId`]s for the given addresses.
    pub fn get_sender_ids(&self, addrs: impl IntoIterator<Item = Address>) -> Vec<SenderId> {
        self.identifiers.write().sender_ids_or_create(addrs)
    }

    /// Returns all senders in the pool
    pub fn unique_senders(&self) -> HashSet<Address> {
        self.get_pool_data().unique_senders()
    }

    /// Converts the changed accounts to a map of sender ids to sender info (internal identifier
    /// used for accounts)
    fn changed_senders(
        &self,
        accs: impl Iterator<Item = ChangedAccount>,
    ) -> FxHashMap<SenderId, SenderInfo> {
        let mut identifiers = self.identifiers.write();
        accs.into_iter()
            .map(|acc| {
                let ChangedAccount { address, nonce, balance } = acc;
                let sender_id = identifiers.sender_id_or_create(address);
                (sender_id, SenderInfo { state_nonce: nonce, balance })
            })
            .collect()
    }

    /// Get the config the pool was configured with.
    pub const fn config(&self) -> &PoolConfig {
        &self.config
    }

    /// Get the validator reference.
    pub const fn validator(&self) -> &V {
        &self.validator
    }

    /// Adds a new transaction listener to the pool that gets notified about every new _pending_
    /// transaction inserted into the pool
    pub fn add_pending_listener(&self, kind: TransactionListenerKind) -> mpsc::Receiver<TxHash> {
        let (sender, rx) = mpsc::channel(self.config.pending_tx_listener_buffer_size);
        let listener = PendingTransactionHashListener { sender, kind };
        self.pending_transaction_listener.lock().push(listener);
        rx
    }

    /// Adds a new transaction listener to the pool that gets notified about every new transaction.
    pub fn add_new_transaction_listener(
        &self,
        kind: TransactionListenerKind,
    ) -> mpsc::Receiver<NewTransactionEvent<T::Transaction>> {
        let (sender, rx) = mpsc::channel(self.config.new_tx_listener_buffer_size);
        let listener = TransactionListener { sender, kind };
        self.transaction_listener.lock().push(listener);
        rx
    }
    /// Adds a new blob sidecar listener to the pool that gets notified about every new
    /// eip4844 transaction's blob sidecar.
    pub fn add_blob_sidecar_listener(&self) -> mpsc::Receiver<NewBlobSidecar> {
        let (sender, rx) = mpsc::channel(BLOB_SIDECAR_LISTENER_BUFFER_SIZE);
        let listener = BlobTransactionSidecarListener { sender };
        self.blob_transaction_sidecar_listener.lock().push(listener);
        rx
    }

    /// If the pool contains the transaction, this adds a new listener that gets notified about
    /// transaction events.
    pub fn add_transaction_event_listener(&self, tx_hash: TxHash) -> Option<TransactionEvents> {
        self.get_pool_data()
            .contains(&tx_hash)
            .then(|| self.event_listener.write().subscribe(tx_hash))
    }

    /// Adds a listener for all transaction events.
    pub fn add_all_transactions_event_listener(&self) -> AllTransactionsEvents<T::Transaction> {
        self.event_listener.write().subscribe_all()
    }

    /// Returns a read lock to the pool's data.
    pub fn get_pool_data(&self) -> RwLockReadGuard<'_, TxPool<T>> {
        self.pool.read()
    }

    /// Returns hashes of transactions in the pool that can be propagated.
    pub fn pooled_transactions_hashes(&self) -> Vec<TxHash> {
        self.get_pool_data()
            .all()
            .transactions_iter()
            .filter(|tx| tx.propagate)
            .map(|tx| *tx.hash())
            .collect()
    }

    /// Returns transactions in the pool that can be propagated
    pub fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().all().transactions_iter().filter(|tx| tx.propagate).cloned().collect()
    }

    /// Returns only the first `max` transactions in the pool that can be propagated.
    pub fn pooled_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data()
            .all()
            .transactions_iter()
            .filter(|tx| tx.propagate)
            .take(max)
            .cloned()
            .collect()
    }

    /// Converts the internally tracked transaction to the pooled format.
    ///
    /// If the transaction is an EIP-4844 transaction, the blob sidecar is fetched from the blob
    /// store and attached to the transaction.
    fn to_pooled_transaction(
        &self,
        transaction: Arc<ValidPoolTransaction<T::Transaction>>,
    ) -> Option<Recovered<<<V as TransactionValidator>::Transaction as PoolTransaction>::Pooled>>
    where
        <V as TransactionValidator>::Transaction: EthPoolTransaction,
    {
        if transaction.is_eip4844() {
            let sidecar = self.blob_store.get(*transaction.hash()).ok()??;
            transaction.transaction.clone().try_into_pooled_eip4844(sidecar)
        } else {
            transaction
                .transaction
                .clone()
                .try_into_pooled()
                .inspect_err(|err| {
                    debug!(
                        target: "txpool", %err,
                        "failed to convert transaction to pooled element; skipping",
                    );
                })
                .ok()
        }
    }

    /// Returns pooled transactions for the given transaction hashes that are allowed to be
    /// propagated.
    pub fn get_pooled_transaction_elements(
        &self,
        tx_hashes: Vec<TxHash>,
        limit: GetPooledTransactionLimit,
    ) -> Vec<<<V as TransactionValidator>::Transaction as PoolTransaction>::Pooled>
    where
        <V as TransactionValidator>::Transaction: EthPoolTransaction,
    {
        let transactions = self.get_all_propagatable(tx_hashes);
        let mut elements = Vec::with_capacity(transactions.len());
        let mut size = 0;
        for transaction in transactions {
            let encoded_len = transaction.encoded_length();
            let Some(pooled) = self.to_pooled_transaction(transaction) else {
                continue;
            };

            size += encoded_len;
            elements.push(pooled.into_inner());

            if limit.exceeds(size) {
                break
            }
        }

        elements
    }

    /// Returns converted pooled transaction for the given transaction hash.
    pub fn get_pooled_transaction_element(
        &self,
        tx_hash: TxHash,
    ) -> Option<Recovered<<<V as TransactionValidator>::Transaction as PoolTransaction>::Pooled>>
    where
        <V as TransactionValidator>::Transaction: EthPoolTransaction,
    {
        self.get(&tx_hash).and_then(|tx| self.to_pooled_transaction(tx))
    }

    /// Updates the entire pool after a new block was executed.
    pub fn on_canonical_state_change<B>(&self, update: CanonicalStateUpdate<'_, B>)
    where
        B: Block,
    {
        trace!(target: "txpool", ?update, "updating pool on canonical state change");

        let block_info = update.block_info();
        let CanonicalStateUpdate {
            new_tip, changed_accounts, mined_transactions, update_kind, ..
        } = update;
        self.validator.on_new_head_block(new_tip);

        let changed_senders = self.changed_senders(changed_accounts.into_iter());

        // update the pool
        let outcome = self.pool.write().on_canonical_state_change(
            block_info,
            mined_transactions,
            changed_senders,
            update_kind,
        );

        // This will discard outdated transactions based on the account's nonce
        self.delete_discarded_blobs(outcome.discarded.iter());

        // notify listeners about updates
        self.notify_on_new_state(outcome);
    }

    /// Performs account updates on the pool.
    ///
    /// This will either promote or discard transactions based on the new account state.
    ///
    /// This should be invoked when the pool drifted and accounts are updated manually
    pub fn update_accounts(&self, accounts: Vec<ChangedAccount>) {
        let changed_senders = self.changed_senders(accounts.into_iter());
        let UpdateOutcome { promoted, discarded } =
            self.pool.write().update_accounts(changed_senders);

        // Notify about promoted pending transactions (similar to notify_on_new_state)
        if !promoted.is_empty() {
            self.pending_transaction_listener.lock().retain_mut(|listener| {
                let promoted_hashes = promoted.iter().filter_map(|tx| {
                    if listener.kind.is_propagate_only() && !tx.propagate {
                        None
                    } else {
                        Some(*tx.hash())
                    }
                });
                listener.send_all(promoted_hashes)
            });

            // in this case we should also emit promoted transactions in full
            self.transaction_listener.lock().retain_mut(|listener| {
                let promoted_txs = promoted.iter().filter_map(|tx| {
                    if listener.kind.is_propagate_only() && !tx.propagate {
                        None
                    } else {
                        Some(NewTransactionEvent::pending(tx.clone()))
                    }
                });
                listener.send_all(promoted_txs)
            });
        }

        {
            let mut listener = self.event_listener.write();
            if !listener.is_empty() {
                for tx in &promoted {
                    listener.pending(tx.hash(), None);
                }
                for tx in &discarded {
                    listener.discarded(tx.hash());
                }
            }
        }

        // This deletes outdated blob txs from the blob store, based on the account's nonce. This is
        // called during txpool maintenance when the pool drifted.
        self.delete_discarded_blobs(discarded.iter());
    }

    /// Add a single validated transaction into the pool.
    ///
    /// Note: this is only used internally by [`Self::add_transactions()`], all new transaction(s)
    /// come in through that function, either as a batch or `std::iter::once`.
    fn add_transaction(
        &self,
        pool: &mut RwLockWriteGuard<'_, TxPool<T>>,
        origin: TransactionOrigin,
        tx: TransactionValidationOutcome<T::Transaction>,
    ) -> PoolResult<AddedTransactionOutcome> {
        match tx {
            TransactionValidationOutcome::Valid {
                balance,
                state_nonce,
                transaction,
                propagate,
                bytecode_hash,
                authorities,
            } => {
                let sender_id = self.get_sender_id(transaction.sender());
                let transaction_id = TransactionId::new(sender_id, transaction.nonce());

                // split the valid transaction and the blob sidecar if it has any
                let (transaction, maybe_sidecar) = match transaction {
                    ValidTransaction::Valid(tx) => (tx, None),
                    ValidTransaction::ValidWithSidecar { transaction, sidecar } => {
                        debug_assert!(
                            transaction.is_eip4844(),
                            "validator returned sidecar for non EIP-4844 transaction"
                        );
                        (transaction, Some(sidecar))
                    }
                };

                let tx = ValidPoolTransaction {
                    transaction,
                    transaction_id,
                    propagate,
                    timestamp: Instant::now(),
                    origin,
                    authority_ids: authorities.map(|auths| self.get_sender_ids(auths)),
                };

                let added = pool.add_transaction(tx, balance, state_nonce, bytecode_hash)?;
                let hash = *added.hash();
                let state = match added.subpool() {
                    SubPool::Pending => AddedTransactionState::Pending,
                    _ => AddedTransactionState::Queued,
                };

                // transaction was successfully inserted into the pool
                if let Some(sidecar) = maybe_sidecar {
                    // notify blob sidecar listeners
                    self.on_new_blob_sidecar(&hash, &sidecar);
                    // store the sidecar in the blob store
                    self.insert_blob(hash, sidecar);
                }

                if let Some(replaced) = added.replaced_blob_transaction() {
                    debug!(target: "txpool", "[{:?}] delete replaced blob sidecar", replaced);
                    // delete the replaced transaction from the blob store
                    self.delete_blob(replaced);
                }

                // Notify about new pending transactions
                if let Some(pending) = added.as_pending() {
                    self.on_new_pending_transaction(pending);
                }

                // Notify tx event listeners
                self.notify_event_listeners(&added);

                if let Some(discarded) = added.discarded_transactions() {
                    self.delete_discarded_blobs(discarded.iter());
                }

                // Notify listeners for _all_ transactions
                self.on_new_transaction(added.into_new_transaction_event());

                Ok(AddedTransactionOutcome { hash, state })
            }
            TransactionValidationOutcome::Invalid(tx, err) => {
                let mut listener = self.event_listener.write();
                listener.invalid(tx.hash());
                Err(PoolError::new(*tx.hash(), err))
            }
            TransactionValidationOutcome::Error(tx_hash, err) => {
                let mut listener = self.event_listener.write();
                listener.discarded(&tx_hash);
                Err(PoolError::other(tx_hash, err))
            }
        }
    }

    /// Adds a transaction and returns the event stream.
    pub fn add_transaction_and_subscribe(
        &self,
        origin: TransactionOrigin,
        tx: TransactionValidationOutcome<T::Transaction>,
    ) -> PoolResult<TransactionEvents> {
        let listener = {
            let mut listener = self.event_listener.write();
            listener.subscribe(tx.tx_hash())
        };
        let mut results = self.add_transactions(origin, std::iter::once(tx));
        results.pop().expect("result length is the same as the input")?;
        Ok(listener)
    }

    /// Adds all transactions in the iterator to the pool, returning a list of results.
    ///
    /// Note: A large batch may lock the pool for a long time that blocks important operations
    /// like updating the pool on canonical state changes. The caller should consider having
    /// a max batch size to balance transaction insertions with other updates.
    pub fn add_transactions(
        &self,
        origin: TransactionOrigin,
        transactions: impl IntoIterator<Item = TransactionValidationOutcome<T::Transaction>>,
    ) -> Vec<PoolResult<AddedTransactionOutcome>> {
        // Add the transactions and enforce the pool size limits in one write lock
        let (mut added, discarded) = {
            let mut pool = self.pool.write();
            let added = transactions
                .into_iter()
                .map(|tx| self.add_transaction(&mut pool, origin, tx))
                .collect::<Vec<_>>();

            // Enforce the pool size limits if at least one transaction was added successfully
            let discarded = if added.iter().any(Result::is_ok) {
                pool.discard_worst()
            } else {
                Default::default()
            };

            (added, discarded)
        };

        if !discarded.is_empty() {
            // Delete any blobs associated with discarded blob transactions
            self.delete_discarded_blobs(discarded.iter());
            self.event_listener.write().discarded_many(&discarded);

            let discarded_hashes =
                discarded.into_iter().map(|tx| *tx.hash()).collect::<HashSet<_>>();

            // A newly added transaction may be immediately discarded, so we need to
            // adjust the result here
            for res in &mut added {
                if let Ok(AddedTransactionOutcome { hash, .. }) = res {
                    if discarded_hashes.contains(hash) {
                        *res = Err(PoolError::new(*hash, PoolErrorKind::DiscardedOnInsert))
                    }
                }
            }
        }

        added
    }

    /// Notify all listeners about a new pending transaction.
    fn on_new_pending_transaction(&self, pending: &AddedPendingTransaction<T::Transaction>) {
        let propagate_allowed = pending.is_propagate_allowed();

        let mut transaction_listeners = self.pending_transaction_listener.lock();
        transaction_listeners.retain_mut(|listener| {
            if listener.kind.is_propagate_only() && !propagate_allowed {
                // only emit this hash to listeners that are only allowed to receive propagate only
                // transactions, such as network
                return !listener.sender.is_closed()
            }

            // broadcast all pending transactions to the listener
            listener.send_all(pending.pending_transactions(listener.kind))
        });
    }

    /// Notify all listeners about a newly inserted pending transaction.
    fn on_new_transaction(&self, event: NewTransactionEvent<T::Transaction>) {
        let mut transaction_listeners = self.transaction_listener.lock();
        transaction_listeners.retain_mut(|listener| {
            if listener.kind.is_propagate_only() && !event.transaction.propagate {
                // only emit this hash to listeners that are only allowed to receive propagate only
                // transactions, such as network
                return !listener.sender.is_closed()
            }

            listener.send(event.clone())
        });
    }

    /// Notify all listeners about a blob sidecar for a newly inserted blob (eip4844) transaction.
    fn on_new_blob_sidecar(&self, tx_hash: &TxHash, sidecar: &BlobTransactionSidecarVariant) {
        let mut sidecar_listeners = self.blob_transaction_sidecar_listener.lock();
        if sidecar_listeners.is_empty() {
            return
        }
        let sidecar = Arc::new(sidecar.clone());
        sidecar_listeners.retain_mut(|listener| {
            let new_blob_event = NewBlobSidecar { tx_hash: *tx_hash, sidecar: sidecar.clone() };
            match listener.sender.try_send(new_blob_event) {
                Ok(()) => true,
                Err(err) => {
                    if matches!(err, mpsc::error::TrySendError::Full(_)) {
                        debug!(
                            target: "txpool",
                            "[{:?}] failed to send blob sidecar; channel full",
                            sidecar,
                        );
                        true
                    } else {
                        false
                    }
                }
            }
        })
    }

    /// Notifies transaction listeners about changes once a block was processed.
    fn notify_on_new_state(&self, outcome: OnNewCanonicalStateOutcome<T::Transaction>) {
        trace!(target: "txpool", promoted=outcome.promoted.len(), discarded= outcome.discarded.len() ,"notifying listeners on state change");

        // notify about promoted pending transactions
        // emit hashes
        self.pending_transaction_listener
            .lock()
            .retain_mut(|listener| listener.send_all(outcome.pending_transactions(listener.kind)));

        // emit full transactions
        self.transaction_listener.lock().retain_mut(|listener| {
            listener.send_all(outcome.full_pending_transactions(listener.kind))
        });

        let OnNewCanonicalStateOutcome { mined, promoted, discarded, block_hash } = outcome;

        // broadcast specific transaction events
        let mut listener = self.event_listener.write();

        if !listener.is_empty() {
            for tx in &mined {
                listener.mined(tx, block_hash);
            }
            for tx in &promoted {
                listener.pending(tx.hash(), None);
            }
            for tx in &discarded {
                listener.discarded(tx.hash());
            }
        }
    }

    /// Fire events for the newly added transaction if there are any.
    fn notify_event_listeners(&self, tx: &AddedTransaction<T::Transaction>) {
        let mut listener = self.event_listener.write();
        if listener.is_empty() {
            // nothing to notify
            return
        }

        match tx {
            AddedTransaction::Pending(tx) => {
                let AddedPendingTransaction { transaction, promoted, discarded, replaced } = tx;

                listener.pending(transaction.hash(), replaced.clone());
                for tx in promoted {
                    listener.pending(tx.hash(), None);
                }
                for tx in discarded {
                    listener.discarded(tx.hash());
                }
            }
            AddedTransaction::Parked { transaction, replaced, .. } => {
                listener.queued(transaction.hash());
                if let Some(replaced) = replaced {
                    listener.replaced(replaced.clone(), *transaction.hash());
                }
            }
        }
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block.
    pub fn best_transactions(&self) -> BestTransactions<T> {
        self.get_pool_data().best_transactions()
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block with
    /// the given base fee and optional blob fee attributes.
    pub fn best_transactions_with_attributes(
        &self,
        best_transactions_attributes: BestTransactionsAttributes,
    ) -> Box<dyn crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<T::Transaction>>>>
    {
        self.get_pool_data().best_transactions_with_attributes(best_transactions_attributes)
    }

    /// Returns only the first `max` transactions in the pending pool.
    pub fn pending_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().pending_transactions_iter().take(max).collect()
    }

    /// Returns all transactions from the pending sub-pool
    pub fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().pending_transactions()
    }

    /// Returns all transactions from parked pools
    pub fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().queued_transactions()
    }

    /// Returns all transactions in the pool
    pub fn all_transactions(&self) -> AllPoolTransactions<T::Transaction> {
        let pool = self.get_pool_data();
        AllPoolTransactions {
            pending: pool.pending_transactions(),
            queued: pool.queued_transactions(),
        }
    }

    /// Returns _all_ transactions in the pool
    pub fn all_transaction_hashes(&self) -> Vec<TxHash> {
        self.get_pool_data().all().transactions_iter().map(|tx| *tx.hash()).collect()
    }

    /// Removes and returns all matching transactions from the pool.
    ///
    /// This behaves as if the transactions got discarded (_not_ mined), effectively introducing a
    /// nonce gap for the given transactions.
    pub fn remove_transactions(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if hashes.is_empty() {
            return Vec::new()
        }
        let removed = self.pool.write().remove_transactions(hashes);

        self.event_listener.write().discarded_many(&removed);

        removed
    }

    /// Removes and returns all matching transactions and their dependent transactions from the
    /// pool.
    pub fn remove_transactions_and_descendants(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if hashes.is_empty() {
            return Vec::new()
        }
        let removed = self.pool.write().remove_transactions_and_descendants(hashes);

        let mut listener = self.event_listener.write();

        for tx in &removed {
            listener.discarded(tx.hash());
        }

        removed
    }

    /// Removes and returns all transactions by the specified sender from the pool.
    pub fn remove_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.get_sender_id(sender);
        let removed = self.pool.write().remove_transactions_by_sender(sender_id);

        self.event_listener.write().discarded_many(&removed);

        removed
    }

    /// Removes and returns all transactions that are present in the pool.
    pub fn retain_unknown<A>(&self, announcement: &mut A)
    where
        A: HandleMempoolData,
    {
        if announcement.is_empty() {
            return
        }
        let pool = self.get_pool_data();
        announcement.retain_by_hash(|tx| !pool.contains(tx))
    }

    /// Returns the transaction by hash.
    pub fn get(&self, tx_hash: &TxHash) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().get(tx_hash)
    }

    /// Returns all transactions of the address
    pub fn get_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.get_sender_id(sender);
        self.get_pool_data().get_transactions_by_sender(sender_id)
    }

    /// Returns all queued transactions of the address by sender
    pub fn get_queued_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.get_sender_id(sender);
        self.get_pool_data().queued_txs_by_sender(sender_id)
    }

    /// Returns all pending transactions filtered by predicate
    pub fn pending_transactions_with_predicate(
        &self,
        predicate: impl FnMut(&ValidPoolTransaction<T::Transaction>) -> bool,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().pending_transactions_with_predicate(predicate)
    }

    /// Returns all pending transactions of the address by sender
    pub fn get_pending_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.get_sender_id(sender);
        self.get_pool_data().pending_txs_by_sender(sender_id)
    }

    /// Returns the highest transaction of the address
    pub fn get_highest_transaction_by_sender(
        &self,
        sender: Address,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.get_sender_id(sender);
        self.get_pool_data().get_highest_transaction_by_sender(sender_id)
    }

    /// Returns the transaction with the highest nonce that is executable given the on chain nonce.
    pub fn get_highest_consecutive_transaction_by_sender(
        &self,
        sender: Address,
        on_chain_nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.get_sender_id(sender);
        self.get_pool_data().get_highest_consecutive_transaction_by_sender(
            sender_id.into_transaction_id(on_chain_nonce),
        )
    }

    /// Returns the transaction given a [`TransactionId`]
    pub fn get_transaction_by_transaction_id(
        &self,
        transaction_id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().all().get(transaction_id).map(|tx| tx.transaction.clone())
    }

    /// Returns all transactions that where submitted with the given [`TransactionOrigin`]
    pub fn get_transactions_by_origin(
        &self,
        origin: TransactionOrigin,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data()
            .all()
            .transactions_iter()
            .filter(|tx| tx.origin == origin)
            .cloned()
            .collect()
    }

    /// Returns all pending transactions filtered by [`TransactionOrigin`]
    pub fn get_pending_transactions_by_origin(
        &self,
        origin: TransactionOrigin,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().pending_transactions_iter().filter(|tx| tx.origin == origin).collect()
    }

    /// Returns all the transactions belonging to the hashes.
    ///
    /// If no transaction exists, it is skipped.
    pub fn get_all(&self, txs: Vec<TxHash>) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if txs.is_empty() {
            return Vec::new()
        }
        self.get_pool_data().get_all(txs).collect()
    }

    /// Returns all the transactions belonging to the hashes that are propagatable.
    ///
    /// If no transaction exists, it is skipped.
    fn get_all_propagatable(
        &self,
        txs: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if txs.is_empty() {
            return Vec::new()
        }
        self.get_pool_data().get_all(txs).filter(|tx| tx.propagate).collect()
    }

    /// Notify about propagated transactions.
    pub fn on_propagated(&self, txs: PropagatedTransactions) {
        if txs.0.is_empty() {
            return
        }
        let mut listener = self.event_listener.write();

        if !listener.is_empty() {
            txs.0.into_iter().for_each(|(hash, peers)| listener.propagated(&hash, peers));
        }
    }

    /// Number of transactions in the entire pool
    pub fn len(&self) -> usize {
        self.get_pool_data().len()
    }

    /// Whether the pool is empty
    pub fn is_empty(&self) -> bool {
        self.get_pool_data().is_empty()
    }

    /// Returns whether or not the pool is over its configured size and transaction count limits.
    pub fn is_exceeded(&self) -> bool {
        self.pool.read().is_exceeded()
    }

    /// Inserts a blob transaction into the blob store
    fn insert_blob(&self, hash: TxHash, blob: BlobTransactionSidecarVariant) {
        debug!(target: "txpool", "[{:?}] storing blob sidecar", hash);
        if let Err(err) = self.blob_store.insert(hash, blob) {
            warn!(target: "txpool", %err, "[{:?}] failed to insert blob", hash);
            self.blob_store_metrics.blobstore_failed_inserts.increment(1);
        }
        self.update_blob_store_metrics();
    }

    /// Delete a blob from the blob store
    pub fn delete_blob(&self, blob: TxHash) {
        let _ = self.blob_store.delete(blob);
    }

    /// Delete all blobs from the blob store
    pub fn delete_blobs(&self, txs: Vec<TxHash>) {
        let _ = self.blob_store.delete_all(txs);
    }

    /// Cleans up the blob store
    pub fn cleanup_blobs(&self) {
        let stat = self.blob_store.cleanup();
        self.blob_store_metrics.blobstore_failed_deletes.increment(stat.delete_failed as u64);
        self.update_blob_store_metrics();
    }

    fn update_blob_store_metrics(&self) {
        if let Some(data_size) = self.blob_store.data_size_hint() {
            self.blob_store_metrics.blobstore_byte_size.set(data_size as f64);
        }
        self.blob_store_metrics.blobstore_entries.set(self.blob_store.blobs_len() as f64);
    }

    /// Deletes all blob transactions that were discarded.
    fn delete_discarded_blobs<'a>(
        &'a self,
        transactions: impl IntoIterator<Item = &'a Arc<ValidPoolTransaction<T::Transaction>>>,
    ) {
        let blob_txs = transactions
            .into_iter()
            .filter(|tx| tx.transaction.is_eip4844())
            .map(|tx| *tx.hash())
            .collect();
        self.delete_blobs(blob_txs);
    }
}

impl<V, T: TransactionOrdering, S> fmt::Debug for PoolInner<V, T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolInner").field("config", &self.config).finish_non_exhaustive()
    }
}

/// Tracks an added transaction and all graph changes caused by adding it.
#[derive(Debug, Clone)]
pub struct AddedPendingTransaction<T: PoolTransaction> {
    /// Inserted transaction.
    transaction: Arc<ValidPoolTransaction<T>>,
    /// Replaced transaction.
    replaced: Option<Arc<ValidPoolTransaction<T>>>,
    /// transactions promoted to the pending queue
    promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// transactions that failed and became discarded
    discarded: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> AddedPendingTransaction<T> {
    /// Returns all transactions that were promoted to the pending pool and adhere to the given
    /// [`TransactionListenerKind`].
    ///
    /// If the kind is [`TransactionListenerKind::PropagateOnly`], then only transactions that
    /// are allowed to be propagated are returned.
    pub(crate) fn pending_transactions(
        &self,
        kind: TransactionListenerKind,
    ) -> impl Iterator<Item = B256> + '_ {
        let iter = std::iter::once(&self.transaction).chain(self.promoted.iter());
        PendingTransactionIter { kind, iter }
    }

    /// Returns if the transaction should be propagated.
    pub(crate) fn is_propagate_allowed(&self) -> bool {
        self.transaction.propagate
    }
}

pub(crate) struct PendingTransactionIter<Iter> {
    kind: TransactionListenerKind,
    iter: Iter,
}

impl<'a, Iter, T> Iterator for PendingTransactionIter<Iter>
where
    Iter: Iterator<Item = &'a Arc<ValidPoolTransaction<T>>>,
    T: PoolTransaction + 'a,
{
    type Item = B256;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let next = self.iter.next()?;
            if self.kind.is_propagate_only() && !next.propagate {
                continue
            }
            return Some(*next.hash())
        }
    }
}

/// An iterator over full pending transactions
pub(crate) struct FullPendingTransactionIter<Iter> {
    kind: TransactionListenerKind,
    iter: Iter,
}

impl<'a, Iter, T> Iterator for FullPendingTransactionIter<Iter>
where
    Iter: Iterator<Item = &'a Arc<ValidPoolTransaction<T>>>,
    T: PoolTransaction + 'a,
{
    type Item = NewTransactionEvent<T>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let next = self.iter.next()?;
            if self.kind.is_propagate_only() && !next.propagate {
                continue
            }
            return Some(NewTransactionEvent {
                subpool: SubPool::Pending,
                transaction: next.clone(),
            })
        }
    }
}

/// Represents a transaction that was added into the pool and its state
#[derive(Debug, Clone)]
pub enum AddedTransaction<T: PoolTransaction> {
    /// Transaction was successfully added and moved to the pending pool.
    Pending(AddedPendingTransaction<T>),
    /// Transaction was successfully added but not yet ready for processing and moved to a
    /// parked pool instead.
    Parked {
        /// Inserted transaction.
        transaction: Arc<ValidPoolTransaction<T>>,
        /// Replaced transaction.
        replaced: Option<Arc<ValidPoolTransaction<T>>>,
        /// The subpool it was moved to.
        subpool: SubPool,
    },
}

impl<T: PoolTransaction> AddedTransaction<T> {
    /// Returns whether the transaction has been added to the pending pool.
    pub(crate) const fn as_pending(&self) -> Option<&AddedPendingTransaction<T>> {
        match self {
            Self::Pending(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the replaced transaction if there was one
    pub(crate) const fn replaced(&self) -> Option<&Arc<ValidPoolTransaction<T>>> {
        match self {
            Self::Pending(tx) => tx.replaced.as_ref(),
            Self::Parked { replaced, .. } => replaced.as_ref(),
        }
    }

    /// Returns the discarded transactions if there were any
    pub(crate) fn discarded_transactions(&self) -> Option<&[Arc<ValidPoolTransaction<T>>]> {
        match self {
            Self::Pending(tx) => Some(&tx.discarded),
            Self::Parked { .. } => None,
        }
    }

    /// Returns the hash of the replaced transaction if it is a blob transaction.
    pub(crate) fn replaced_blob_transaction(&self) -> Option<B256> {
        self.replaced().filter(|tx| tx.transaction.is_eip4844()).map(|tx| *tx.transaction.hash())
    }

    /// Returns the hash of the transaction
    pub(crate) fn hash(&self) -> &TxHash {
        match self {
            Self::Pending(tx) => tx.transaction.hash(),
            Self::Parked { transaction, .. } => transaction.hash(),
        }
    }

    /// Converts this type into the event type for listeners
    pub(crate) fn into_new_transaction_event(self) -> NewTransactionEvent<T> {
        match self {
            Self::Pending(tx) => {
                NewTransactionEvent { subpool: SubPool::Pending, transaction: tx.transaction }
            }
            Self::Parked { transaction, subpool, .. } => {
                NewTransactionEvent { transaction, subpool }
            }
        }
    }

    /// Returns the subpool this transaction was added to
    pub(crate) const fn subpool(&self) -> SubPool {
        match self {
            Self::Pending(_) => SubPool::Pending,
            Self::Parked { subpool, .. } => *subpool,
        }
    }

    /// Returns the [`TransactionId`] of the added transaction
    #[cfg(test)]
    pub(crate) fn id(&self) -> &TransactionId {
        match self {
            Self::Pending(added) => added.transaction.id(),
            Self::Parked { transaction, .. } => transaction.id(),
        }
    }
}

/// The state of a transaction when is was added to the pool
#[derive(Debug)]
pub enum AddedTransactionState {
    /// Ready for execution
    Pending,
    /// Not ready for execution due to a nonce gap or insufficient balance
    Queued, // TODO: Break it down to missing nonce, insufficient balance, etc.
}

/// The outcome of a successful transaction addition
#[derive(Debug)]
pub struct AddedTransactionOutcome {
    /// The hash of the transaction
    pub hash: TxHash,
    /// The state of the transaction
    pub state: AddedTransactionState,
}

/// Contains all state changes after a [`CanonicalStateUpdate`] was processed
#[derive(Debug)]
pub(crate) struct OnNewCanonicalStateOutcome<T: PoolTransaction> {
    /// Hash of the block.
    pub(crate) block_hash: B256,
    /// All mined transactions.
    pub(crate) mined: Vec<TxHash>,
    /// Transactions promoted to the pending pool.
    pub(crate) promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// transaction that were discarded during the update
    pub(crate) discarded: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> OnNewCanonicalStateOutcome<T> {
    /// Returns all transactions that were promoted to the pending pool and adhere to the given
    /// [`TransactionListenerKind`].
    ///
    /// If the kind is [`TransactionListenerKind::PropagateOnly`], then only transactions that
    /// are allowed to be propagated are returned.
    pub(crate) fn pending_transactions(
        &self,
        kind: TransactionListenerKind,
    ) -> impl Iterator<Item = B256> + '_ {
        let iter = self.promoted.iter();
        PendingTransactionIter { kind, iter }
    }

    /// Returns all FULL transactions that were promoted to the pending pool and adhere to the given
    /// [`TransactionListenerKind`].
    ///
    /// If the kind is [`TransactionListenerKind::PropagateOnly`], then only transactions that
    /// are allowed to be propagated are returned.
    pub(crate) fn full_pending_transactions(
        &self,
        kind: TransactionListenerKind,
    ) -> impl Iterator<Item = NewTransactionEvent<T>> + '_ {
        let iter = self.promoted.iter();
        FullPendingTransactionIter { kind, iter }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        blobstore::{BlobStore, InMemoryBlobStore},
        identifier::SenderId,
        test_utils::{MockTransaction, TestPoolBuilder},
        validate::ValidTransaction,
        BlockInfo, PoolConfig, SubPoolLimit, TransactionOrigin, TransactionValidationOutcome, U256,
    };
    use alloy_eips::{eip4844::BlobTransactionSidecar, eip7594::BlobTransactionSidecarVariant};
    use alloy_primitives::Address;
    use std::{fs, path::PathBuf};

    #[test]
    fn test_discard_blobs_on_blob_tx_eviction() {
        let blobs = {
            // Read the contents of the JSON file into a string.
            let json_content = fs::read_to_string(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/blob1.json"),
            )
            .expect("Failed to read the blob data file");

            // Parse the JSON contents into a serde_json::Value.
            let json_value: serde_json::Value =
                serde_json::from_str(&json_content).expect("Failed to deserialize JSON");

            // Extract blob data from JSON and convert it to Blob.
            vec![
                // Extract the "data" field from the JSON and parse it as a string.
                json_value
                    .get("data")
                    .unwrap()
                    .as_str()
                    .expect("Data is not a valid string")
                    .to_string(),
            ]
        };

        // Generate a BlobTransactionSidecar from the blobs.
        let sidecar = BlobTransactionSidecarVariant::Eip4844(
            BlobTransactionSidecar::try_from_blobs_hex(blobs).unwrap(),
        );

        // Define the maximum limit for blobs in the sub-pool.
        let blob_limit = SubPoolLimit::new(1000, usize::MAX);

        // Create a test pool with default configuration and the specified blob limit.
        let test_pool = &TestPoolBuilder::default()
            .with_config(PoolConfig { blob_limit, ..Default::default() })
            .pool;

        // Set the block info for the pool, including a pending blob fee.
        test_pool
            .set_block_info(BlockInfo { pending_blob_fee: Some(10_000_000), ..Default::default() });

        // Create an in-memory blob store.
        let blob_store = InMemoryBlobStore::default();

        // Loop to add transactions to the pool and test blob eviction.
        for n in 0..blob_limit.max_txs + 10 {
            // Create a mock transaction with the generated blob sidecar.
            let mut tx = MockTransaction::eip4844_with_sidecar(sidecar.clone());

            // Set non zero size
            tx.set_size(1844674407370951);

            // Insert the sidecar into the blob store if the current index is within the blob limit.
            if n < blob_limit.max_txs {
                blob_store.insert(*tx.get_hash(), sidecar.clone()).unwrap();
            }

            // Add the transaction to the pool with external origin and valid outcome.
            test_pool.add_transactions(
                TransactionOrigin::External,
                [TransactionValidationOutcome::Valid {
                    balance: U256::from(1_000),
                    state_nonce: 0,
                    bytecode_hash: None,
                    transaction: ValidTransaction::ValidWithSidecar {
                        transaction: tx,
                        sidecar: sidecar.clone(),
                    },
                    propagate: true,
                    authorities: None,
                }],
            );
        }

        // Assert that the size of the pool's blob component is equal to the maximum blob limit.
        assert_eq!(test_pool.size().blob, blob_limit.max_txs);

        // Assert that the size of the pool's blob_size component matches the expected value.
        assert_eq!(test_pool.size().blob_size, 1844674407370951000);

        // Assert that the pool's blob store matches the expected blob store.
        assert_eq!(*test_pool.blob_store(), blob_store);
    }

    #[test]
    fn test_auths_stored_in_identifiers() {
        // Create a test pool with default configuration.
        let test_pool = &TestPoolBuilder::default().with_config(Default::default()).pool;

        let auth = Address::new([1; 20]);
        let tx = MockTransaction::eip7702();

        test_pool.add_transactions(
            TransactionOrigin::Local,
            [TransactionValidationOutcome::Valid {
                balance: U256::from(1_000),
                state_nonce: 0,
                bytecode_hash: None,
                transaction: ValidTransaction::Valid(tx),
                propagate: true,
                authorities: Some(vec![auth]),
            }],
        );

        let identifiers = test_pool.identifiers.read();
        assert_eq!(identifiers.sender_id(&auth), Some(SenderId::from(1)));
    }
}
