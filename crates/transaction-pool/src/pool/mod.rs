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
//!  - Pending Pool: Contains all transactions that are valid on the current state and satisfy
//! (3. a)(1): _No_ nonce gaps. A _pending_ transaction is considered _ready_ when it has the lowest
//! nonce of all transactions from the same sender. Once a _ready_ transaction with nonce `n` has
//! been executed, the next highest transaction from the same sender `n + 1` becomes ready.
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
//!  - _Pending_: pending transactions are transactions that fall under (2.). These transactions can
//!    currently be executed and are stored in the pending sub-pool
//!  - _Queued_: queued transactions are transactions that fall under category (3.). Those
//!    transactions are _currently_ waiting for state changes that eventually move them into
//!    category (2.) and become pending.

use crate::{
    error::{PoolError, PoolResult},
    identifier::{SenderId, SenderIdentifiers, TransactionId},
    pool::{
        listener::PoolEventBroadcast,
        state::SubPool,
        txpool::{SenderInfo, TxPool},
    },
    traits::{
        AllPoolTransactions, BestTransactionsAttributes, BlockInfo, NewTransactionEvent, PoolSize,
        PoolTransaction, PropagatedTransactions, TransactionOrigin,
    },
    validate::{TransactionValidationOutcome, ValidPoolTransaction},
    CanonicalStateUpdate, ChangedAccount, PoolConfig, TransactionOrdering, TransactionValidator,
};
use best::BestTransactions;
use parking_lot::{Mutex, RwLock};
use reth_primitives::{
    Address, BlobTransaction, BlobTransactionSidecar, IntoRecoveredTransaction,
    PooledTransactionsElement, TransactionSigned, TxHash, B256,
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
    time::Instant,
};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};

mod events;
pub use events::{FullTransactionEvent, TransactionEvent};

mod listener;
use crate::{
    blobstore::BlobStore,
    metrics::BlobStoreMetrics,
    pool::txpool::UpdateOutcome,
    traits::{GetPooledTransactionLimit, NewBlobSidecar, TransactionListenerKind},
    validate::ValidTransaction,
};
use alloy_rlp::Encodable;
pub use listener::{AllTransactionsEvents, TransactionEvents};

mod best;
mod blob;
mod parked;
pub(crate) mod pending;
pub(crate) mod size;
pub(crate) mod state;
pub mod txpool;
mod update;

const PENDING_TX_LISTENER_BUFFER_SIZE: usize = 2048;
const NEW_TX_LISTENER_BUFFER_SIZE: usize = 1024;
const BLOB_SIDECAR_LISTENER_BUFFER_SIZE: usize = 512;

/// Transaction pool internals.
pub struct PoolInner<V, T, S>
where
    T: TransactionOrdering,
{
    /// Internal mapping of addresses to plain ints.
    identifiers: RwLock<SenderIdentifiers>,
    /// Transaction validation.
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
    pub(crate) fn new(validator: V, ordering: T, blob_store: S, config: PoolConfig) -> Self {
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
    pub(crate) fn blob_store(&self) -> &S {
        &self.blob_store
    }

    /// Returns stats about the size of the pool.
    pub(crate) fn size(&self) -> PoolSize {
        self.pool.read().size()
    }

    /// Returns the currently tracked block
    pub(crate) fn block_info(&self) -> BlockInfo {
        self.pool.read().block_info()
    }
    /// Returns the currently tracked block
    pub(crate) fn set_block_info(&self, info: BlockInfo) {
        self.pool.write().set_block_info(info)
    }

    /// Returns the internal `SenderId` for this address
    pub(crate) fn get_sender_id(&self, addr: Address) -> SenderId {
        self.identifiers.write().sender_id_or_create(addr)
    }

    /// Returns all senders in the pool
    pub(crate) fn unique_senders(&self) -> HashSet<Address> {
        self.pool.read().unique_senders()
    }

    /// Converts the changed accounts to a map of sender ids to sender info (internal identifier
    /// used for accounts)
    fn changed_senders(
        &self,
        accs: impl Iterator<Item = ChangedAccount>,
    ) -> HashMap<SenderId, SenderInfo> {
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
    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    /// Get the validator reference.
    pub fn validator(&self) -> &V {
        &self.validator
    }

    /// Adds a new transaction listener to the pool that gets notified about every new _pending_
    /// transaction inserted into the pool
    pub fn add_pending_listener(&self, kind: TransactionListenerKind) -> mpsc::Receiver<TxHash> {
        let (sender, rx) = mpsc::channel(PENDING_TX_LISTENER_BUFFER_SIZE);
        let listener = PendingTransactionHashListener { sender, kind };
        self.pending_transaction_listener.lock().push(listener);
        rx
    }

    /// Adds a new transaction listener to the pool that gets notified about every new transaction.
    pub fn add_new_transaction_listener(
        &self,
        kind: TransactionListenerKind,
    ) -> mpsc::Receiver<NewTransactionEvent<T::Transaction>> {
        let (sender, rx) = mpsc::channel(NEW_TX_LISTENER_BUFFER_SIZE);
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
    pub(crate) fn add_transaction_event_listener(
        &self,
        tx_hash: TxHash,
    ) -> Option<TransactionEvents> {
        let pool = self.pool.read();
        if pool.contains(&tx_hash) {
            Some(self.event_listener.write().subscribe(tx_hash))
        } else {
            None
        }
    }

    /// Adds a listener for all transaction events.
    pub(crate) fn add_all_transactions_event_listener(
        &self,
    ) -> AllTransactionsEvents<T::Transaction> {
        self.event_listener.write().subscribe_all()
    }

    /// Returns hashes of _all_ transactions in the pool.
    pub(crate) fn pooled_transactions_hashes(&self) -> Vec<TxHash> {
        let pool = self.pool.read();
        pool.all().transactions_iter().filter(|tx| tx.propagate).map(|tx| *tx.hash()).collect()
    }

    /// Returns _all_ transactions in the pool.
    pub(crate) fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let pool = self.pool.read();
        pool.all().transactions_iter().filter(|tx| tx.propagate).collect()
    }

    /// Returns the [BlobTransaction] for the given transaction if the sidecar exists.
    ///
    /// Caution: this assumes the given transaction is eip-4844
    fn get_blob_transaction(&self, transaction: TransactionSigned) -> Option<BlobTransaction> {
        if let Ok(Some(sidecar)) = self.blob_store.get(transaction.hash()) {
            if let Ok(blob) = BlobTransaction::try_from_signed(transaction, sidecar) {
                return Some(blob)
            }
        }
        None
    }

    /// Returns converted [PooledTransactionsElement] for the given transaction hashes.
    pub(crate) fn get_pooled_transaction_elements(
        &self,
        tx_hashes: Vec<TxHash>,
        limit: GetPooledTransactionLimit,
    ) -> Vec<PooledTransactionsElement> {
        let transactions = self.get_all(tx_hashes);
        let mut elements = Vec::with_capacity(transactions.len());
        let mut size = 0;
        for transaction in transactions {
            let tx = transaction.to_recovered_transaction().into_signed();
            let pooled = if tx.is_eip4844() {
                if let Some(blob) = self.get_blob_transaction(tx) {
                    PooledTransactionsElement::BlobTransaction(blob)
                } else {
                    continue
                }
            } else {
                PooledTransactionsElement::from(tx)
            };

            size += pooled.length();
            elements.push(pooled);

            if limit.exceeds(size) {
                break
            }
        }

        elements
    }

    /// Updates the entire pool after a new block was executed.
    pub(crate) fn on_canonical_state_change(&self, update: CanonicalStateUpdate<'_>) {
        trace!(target: "txpool", %update, "updating pool on canonical state change");

        let block_info = update.block_info();
        let CanonicalStateUpdate { new_tip, changed_accounts, mined_transactions, .. } = update;
        self.validator.on_new_head_block(new_tip);

        let changed_senders = self.changed_senders(changed_accounts.into_iter());

        // update the pool
        let outcome = self.pool.write().on_canonical_state_change(
            block_info,
            mined_transactions,
            changed_senders,
        );

        // This will discard outdated transactions based on the account's nonce
        self.delete_discarded_blobs(outcome.discarded.iter());

        // notify listeners about updates
        self.notify_on_new_state(outcome);
    }

    /// Performs account updates on the pool.
    ///
    /// This will either promote or discard transactions based on the new account state.
    pub(crate) fn update_accounts(&self, accounts: Vec<ChangedAccount>) {
        let changed_senders = self.changed_senders(accounts.into_iter());
        let UpdateOutcome { promoted, discarded } =
            self.pool.write().update_accounts(changed_senders);
        let mut listener = self.event_listener.write();

        promoted.iter().for_each(|tx| listener.pending(tx.hash(), None));
        discarded.iter().for_each(|tx| listener.discarded(tx.hash()));

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
        origin: TransactionOrigin,
        tx: TransactionValidationOutcome<T::Transaction>,
    ) -> PoolResult<TxHash> {
        match tx {
            TransactionValidationOutcome::Valid {
                balance,
                state_nonce,
                transaction,
                propagate,
            } => {
                let sender_id = self.get_sender_id(transaction.sender());
                let transaction_id = TransactionId::new(sender_id, transaction.nonce());
                let encoded_length = transaction.encoded_length();

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
                    encoded_length,
                };

                let added = self.pool.write().add_transaction(tx, balance, state_nonce)?;
                let hash = *added.hash();

                // transaction was successfully inserted into the pool
                if let Some(sidecar) = maybe_sidecar {
                    // notify blob sidecar listeners
                    self.on_new_blob_sidecar(&hash, &sidecar);
                    // store the sidecar in the blob store
                    self.insert_blob(hash, sidecar);
                }

                if let Some(replaced) = added.replaced_blob_transaction() {
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

                Ok(hash)
            }
            TransactionValidationOutcome::Invalid(tx, err) => {
                let mut listener = self.event_listener.write();
                listener.discarded(tx.hash());
                Err(PoolError::InvalidTransaction(*tx.hash(), err))
            }
            TransactionValidationOutcome::Error(tx_hash, err) => {
                let mut listener = self.event_listener.write();
                listener.discarded(&tx_hash);
                Err(PoolError::Other(tx_hash, err))
            }
        }
    }

    pub(crate) fn add_transaction_and_subscribe(
        &self,
        origin: TransactionOrigin,
        tx: TransactionValidationOutcome<T::Transaction>,
    ) -> PoolResult<TransactionEvents> {
        let listener = {
            let mut listener = self.event_listener.write();
            listener.subscribe(tx.tx_hash())
        };
        self.add_transactions(origin, std::iter::once(tx)).pop().expect("exists; qed")?;
        Ok(listener)
    }

    /// Adds all transactions in the iterator to the pool, returning a list of results.
    pub fn add_transactions(
        &self,
        origin: TransactionOrigin,
        transactions: impl IntoIterator<Item = TransactionValidationOutcome<T::Transaction>>,
    ) -> Vec<PoolResult<TxHash>> {
        let added =
            transactions.into_iter().map(|tx| self.add_transaction(origin, tx)).collect::<Vec<_>>();

        // If at least one transaction was added successfully, then we enforce the pool size limits.
        let discarded =
            if added.iter().any(Result::is_ok) { self.discard_worst() } else { Default::default() };

        if discarded.is_empty() {
            return added
        }

        let mut listener = self.event_listener.write();
        discarded.iter().for_each(|tx| listener.discarded(tx));

        // It may happen that a newly added transaction is immediately discarded, so we need to
        // adjust the result here
        added
            .into_iter()
            .map(|res| match res {
                Ok(ref hash) if discarded.contains(hash) => {
                    Err(PoolError::DiscardedOnInsert(*hash))
                }
                other => other,
            })
            .collect()
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
    fn on_new_blob_sidecar(&self, tx_hash: &TxHash, sidecar: &BlobTransactionSidecar) {
        let mut sidecar_listeners = self.blob_transaction_sidecar_listener.lock();
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

    /// Notifies transaction listeners about changes after a block was processed.
    fn notify_on_new_state(&self, outcome: OnNewCanonicalStateOutcome<T::Transaction>) {
        // notify about promoted pending transactions
        {
            let mut transaction_listeners = self.pending_transaction_listener.lock();
            transaction_listeners.retain_mut(|listener| {
                listener.send_all(outcome.pending_transactions(listener.kind))
            });
        }

        let OnNewCanonicalStateOutcome { mined, promoted, discarded, block_hash } = outcome;

        let mut listener = self.event_listener.write();

        mined.iter().for_each(|tx| listener.mined(tx, block_hash));
        promoted.iter().for_each(|tx| listener.pending(tx.hash(), None));
        discarded.iter().for_each(|tx| listener.discarded(tx.hash()));
    }

    /// Fire events for the newly added transaction if there are any.
    fn notify_event_listeners(&self, tx: &AddedTransaction<T::Transaction>) {
        let mut listener = self.event_listener.write();

        match tx {
            AddedTransaction::Pending(tx) => {
                let AddedPendingTransaction { transaction, promoted, discarded, replaced } = tx;

                listener.pending(transaction.hash(), replaced.clone());
                promoted.iter().for_each(|tx| listener.pending(tx.hash(), None));
                discarded.iter().for_each(|tx| listener.discarded(tx.hash()));
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
    pub(crate) fn best_transactions(&self) -> BestTransactions<T> {
        self.pool.read().best_transactions()
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block with
    /// the given base fee.
    pub(crate) fn best_transactions_with_base_fee(
        &self,
        base_fee: u64,
    ) -> Box<dyn crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<T::Transaction>>>>
    {
        self.pool.read().best_transactions_with_base_fee(base_fee)
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block with
    /// the given base fee and optional blob fee attributes.
    pub(crate) fn best_transactions_with_attributes(
        &self,
        best_transactions_attributes: BestTransactionsAttributes,
    ) -> Box<dyn crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<T::Transaction>>>>
    {
        self.pool.read().best_transactions_with_attributes(best_transactions_attributes)
    }

    /// Returns all transactions from the pending sub-pool
    pub(crate) fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.pool.read().pending_transactions()
    }

    /// Returns all transactions from parked pools
    pub(crate) fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.pool.read().queued_transactions()
    }

    /// Returns all transactions in the pool
    pub(crate) fn all_transactions(&self) -> AllPoolTransactions<T::Transaction> {
        let pool = self.pool.read();
        AllPoolTransactions {
            pending: pool.pending_transactions(),
            queued: pool.queued_transactions(),
        }
    }

    /// Removes and returns all matching transactions from the pool.
    pub(crate) fn remove_transactions(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if hashes.is_empty() {
            return Vec::new()
        }
        let removed = self.pool.write().remove_transactions(hashes);

        let mut listener = self.event_listener.write();

        removed.iter().for_each(|tx| listener.discarded(tx.hash()));

        removed
    }

    /// Removes all transactions that are present in the pool.
    pub(crate) fn retain_unknown(&self, hashes: &mut Vec<TxHash>) {
        if hashes.is_empty() {
            return
        }
        let pool = self.pool.read();
        hashes.retain(|tx| !pool.contains(tx))
    }

    /// Returns the transaction by hash.
    pub(crate) fn get(
        &self,
        tx_hash: &TxHash,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.pool.read().get(tx_hash)
    }

    /// Returns all transactions of the address
    pub(crate) fn get_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.get_sender_id(sender);
        self.pool.read().get_transactions_by_sender(sender_id)
    }

    /// Returns all the transactions belonging to the hashes.
    ///
    /// If no transaction exists, it is skipped.
    pub(crate) fn get_all(
        &self,
        txs: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if txs.is_empty() {
            return Vec::new()
        }
        self.pool.read().get_all(txs).collect()
    }

    /// Notify about propagated transactions.
    pub(crate) fn on_propagated(&self, txs: PropagatedTransactions) {
        if txs.0.is_empty() {
            return
        }
        let mut listener = self.event_listener.write();

        txs.0.into_iter().for_each(|(hash, peers)| listener.propagated(&hash, peers))
    }

    /// Number of transactions in the entire pool
    pub(crate) fn len(&self) -> usize {
        self.pool.read().len()
    }

    /// Whether the pool is empty
    pub(crate) fn is_empty(&self) -> bool {
        self.pool.read().is_empty()
    }

    /// Enforces the size limits of pool and returns the discarded transactions if violated.
    pub(crate) fn discard_worst(&self) -> HashSet<TxHash> {
        self.pool.write().discard_worst().into_iter().map(|tx| *tx.hash()).collect()
    }

    /// Inserts a blob transaction into the blob store
    fn insert_blob(&self, hash: TxHash, blob: BlobTransactionSidecar) {
        if let Err(err) = self.blob_store.insert(hash, blob) {
            warn!(target: "txpool", ?err, "[{:?}] failed to insert blob", hash);
            self.blob_store_metrics.blobstore_failed_inserts.increment(1);
        }
        self.update_blob_store_metrics();
    }

    /// Delete a blob from the blob store
    pub(crate) fn delete_blob(&self, blob: TxHash) {
        if let Err(err) = self.blob_store.delete(blob) {
            warn!(target: "txpool", ?err, "[{:?}] failed to delete blobs", blob);
            self.blob_store_metrics.blobstore_failed_deletes.increment(1);
        }
        self.update_blob_store_metrics();
    }

    /// Delete all blobs from the blob store
    pub(crate) fn delete_blobs(&self, txs: Vec<TxHash>) {
        let num = txs.len();
        if let Err(err) = self.blob_store.delete_all(txs) {
            warn!(target: "txpool", ?err,?num, "failed to delete blobs");
            self.blob_store_metrics.blobstore_failed_deletes.increment(num as u64);
        }
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

/// An active listener for new pending transactions.
#[derive(Debug)]
struct PendingTransactionHashListener {
    sender: mpsc::Sender<TxHash>,
    /// Whether to include transactions that should not be propagated over the network.
    kind: TransactionListenerKind,
}

impl PendingTransactionHashListener {
    /// Attempts to send all hashes to the listener.
    ///
    /// Returns false if the channel is closed (receiver dropped)
    fn send_all(&self, hashes: impl IntoIterator<Item = TxHash>) -> bool {
        for tx_hash in hashes.into_iter() {
            match self.sender.try_send(tx_hash) {
                Ok(()) => {}
                Err(err) => {
                    return if matches!(err, mpsc::error::TrySendError::Full(_)) {
                        debug!(
                            target: "txpool",
                            "[{:?}] failed to send pending tx; channel full",
                            tx_hash,
                        );
                        true
                    } else {
                        false
                    }
                }
            }
        }
        true
    }
}

/// An active listener for new pending transactions.
#[derive(Debug)]
struct TransactionListener<T: PoolTransaction> {
    sender: mpsc::Sender<NewTransactionEvent<T>>,
    /// Whether to include transactions that should not be propagated over the network.
    kind: TransactionListenerKind,
}

impl<T: PoolTransaction> TransactionListener<T> {
    /// Attempts to send the event to the listener.
    ///
    /// Returns false if the channel is closed (receiver dropped)
    fn send(&self, event: NewTransactionEvent<T>) -> bool {
        self.send_all(std::iter::once(event))
    }

    /// Attempts to send all events to the listener.
    ///
    /// Returns false if the channel is closed (receiver dropped)
    fn send_all(&self, events: impl IntoIterator<Item = NewTransactionEvent<T>>) -> bool {
        for event in events.into_iter() {
            match self.sender.try_send(event) {
                Ok(()) => {}
                Err(err) => {
                    return if let mpsc::error::TrySendError::Full(event) = err {
                        debug!(
                            target: "txpool",
                            "[{:?}] failed to send pending tx; channel full",
                            event.transaction.hash(),
                        );
                        true
                    } else {
                        false
                    }
                }
            }
        }
        true
    }
}

/// An active listener for new blobs
#[derive(Debug)]
struct BlobTransactionSidecarListener {
    sender: mpsc::Sender<NewBlobSidecar>,
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
    /// [TransactionListenerKind].
    ///
    /// If the kind is [TransactionListenerKind::PropagateOnly], then only transactions that
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
    pub(crate) fn as_pending(&self) -> Option<&AddedPendingTransaction<T>> {
        match self {
            AddedTransaction::Pending(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the replaced transaction if there was one
    pub(crate) fn replaced(&self) -> Option<&Arc<ValidPoolTransaction<T>>> {
        match self {
            AddedTransaction::Pending(tx) => tx.replaced.as_ref(),
            AddedTransaction::Parked { replaced, .. } => replaced.as_ref(),
        }
    }

    /// Returns the discarded transactions if there were any
    pub(crate) fn discarded_transactions(&self) -> Option<&[Arc<ValidPoolTransaction<T>>]> {
        match self {
            AddedTransaction::Pending(tx) => Some(&tx.discarded),
            AddedTransaction::Parked { .. } => None,
        }
    }

    /// Returns the hash of the replaced transaction if it is a blob transaction.
    pub(crate) fn replaced_blob_transaction(&self) -> Option<B256> {
        self.replaced().filter(|tx| tx.transaction.is_eip4844()).map(|tx| *tx.transaction.hash())
    }

    /// Returns the hash of the transaction
    pub(crate) fn hash(&self) -> &TxHash {
        match self {
            AddedTransaction::Pending(tx) => tx.transaction.hash(),
            AddedTransaction::Parked { transaction, .. } => transaction.hash(),
        }
    }

    /// Converts this type into the event type for listeners
    pub(crate) fn into_new_transaction_event(self) -> NewTransactionEvent<T> {
        match self {
            AddedTransaction::Pending(tx) => {
                NewTransactionEvent { subpool: SubPool::Pending, transaction: tx.transaction }
            }
            AddedTransaction::Parked { transaction, subpool, .. } => {
                NewTransactionEvent { transaction, subpool }
            }
        }
    }

    /// Returns the subpool this transaction was added to
    #[cfg(test)]
    pub(crate) fn subpool(&self) -> SubPool {
        match self {
            AddedTransaction::Pending(_) => SubPool::Pending,
            AddedTransaction::Parked { subpool, .. } => *subpool,
        }
    }

    /// Returns the [TransactionId] of the added transaction
    #[cfg(test)]
    pub(crate) fn id(&self) -> &TransactionId {
        match self {
            AddedTransaction::Pending(added) => added.transaction.id(),
            AddedTransaction::Parked { transaction, .. } => transaction.id(),
        }
    }
}

/// Contains all state changes after a [`CanonicalStateUpdate`] was processed
#[derive(Debug)]
pub(crate) struct OnNewCanonicalStateOutcome<T: PoolTransaction> {
    /// Hash of the block.
    pub(crate) block_hash: B256,
    /// All mined transactions.
    pub(crate) mined: Vec<TxHash>,
    /// Transactions promoted to the ready queue.
    pub(crate) promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// transaction that were discarded during the update
    pub(crate) discarded: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> OnNewCanonicalStateOutcome<T> {
    /// Returns all transactions that were promoted to the pending pool and adhere to the given
    /// [TransactionListenerKind].
    ///
    /// If the kind is [TransactionListenerKind::PropagateOnly], then only transactions that
    /// are allowed to be propagated are returned.
    pub(crate) fn pending_transactions(
        &self,
        kind: TransactionListenerKind,
    ) -> impl Iterator<Item = B256> + '_ {
        let iter = self.promoted.iter();
        PendingTransactionIter { kind, iter }
    }
}
