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
    blobstore::{BlobStore, PooledBlobSidecar},
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
    validate::{
        FrameDependencies, TransactionValidationOutcome, ValidPoolTransaction, ValidTransaction,
    },
    CanonicalStateUpdate, EthPoolTransaction, PoolConfig, SubPoolLimit, TransactionOrdering,
    TransactionValidator,
};
use alloy_consensus::Transaction;

use alloy_primitives::{
    map::{AddressSet, B256Set},
    Address, TxHash, B256,
};
use parking_lot::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use reth_eth_wire_types::HandleMempoolData;
use reth_execution_types::ChangedAccount;

use alloy_eips::eip7594::BlobTransactionSidecarVariant;
use reth_primitives_traits::Recovered;
use rustc_hash::FxHashMap;
use std::{
    fmt,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::sync::mpsc;
use tracing::{debug, trace, warn};
mod events;
mod frame_revalidation;
pub use best::{BestTransactionFilter, BestTransactionsWithPrioritizedSenders};
pub use blob::{blob_tx_priority, fee_delta, BlobOrd, BlobTransactions};
pub use events::{FullTransactionEvent, NewTransactionEvent, TransactionEvent};
use frame_revalidation::FrameRevalidationQueue;
pub use listener::{AllTransactionsEvents, TransactionEvents, TransactionListenerKind};
pub use parked::{BasefeeOrd, ParkedOrd, ParkedPool, QueuedOrd};
pub use pending::PendingPool;

mod best;
pub use best::BestTransactions;

mod blob;
pub mod listener;
mod parked;
pub mod pending;
pub mod size;
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
    /// Withdrawn frames awaiting revalidation; always lock after `pool` when holding both.
    frame_revalidation: Mutex<FrameRevalidationQueue<T::Transaction>>,
    /// Pool settings.
    config: PoolConfig,
    /// Manages listeners for transaction state change events.
    event_listener: RwLock<PoolEventBroadcast<T::Transaction>>,
    /// Tracks whether any event listeners have ever been installed.
    has_event_listeners: AtomicBool,
    /// Listeners for new _full_ pending transactions.
    pending_transaction_listener: RwLock<Vec<PendingTransactionHashListener>>,
    /// Listeners for new transactions added to the pool.
    transaction_listener: RwLock<Vec<TransactionListener<T::Transaction>>>,
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
            has_event_listeners: AtomicBool::new(false),
            pool: RwLock::new(TxPool::new(ordering, config.clone())),
            frame_revalidation: Default::default(),
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
    /// Sets the currently tracked block.
    ///
    /// This will also notify subscribers about any transactions that were promoted to the pending
    /// pool due to fee changes.
    /// A head change withdraws public frames until [`Self::revalidate_frame_transactions`] runs.
    pub fn set_block_info(&self, info: BlockInfo) {
        let outcome = {
            let mut pool = self.pool.write();
            let mut discarded = Vec::new();
            if pool.block_info().last_seen_block_hash != info.last_seen_block_hash {
                let hashes = pool.frame_transaction_hashes();
                let mut queue = self.frame_revalidation.lock();
                for tx in pool.remove_transactions(hashes) {
                    discarded.extend(queue.insert(tx));
                }
                discarded.extend(queue.discard_worst(self.frame_revalidation_limit()));
            }
            let mut outcome = pool.set_block_info(info);
            outcome.discarded.extend(discarded);
            outcome
        };

        // Notify subscribers about promoted transactions due to fee changes
        self.notify_on_transaction_updates(outcome.promoted, outcome.discarded);
    }

    /// Returns the internal [`SenderId`] for this address, allocating a new mapping when the
    /// address is first observed.
    ///
    /// This must only be used on paths that intentionally begin tracking a sender, such as
    /// transaction insertion. Read-only lookups should prefer [`Self::sender_id`] to avoid
    /// growing the sender-id map for unknown addresses.
    pub fn get_sender_id(&self, addr: Address) -> SenderId {
        self.identifiers.write().sender_id_or_create(addr)
    }

    /// Returns the internal [`SenderId`] for this address if it is already tracked.
    ///
    /// Unlike [`Self::get_sender_id`], this never allocates a new sender mapping and is therefore
    /// suitable for read-only queries or best-effort cleanup on unknown addresses.
    pub fn sender_id(&self, addr: &Address) -> Option<SenderId> {
        self.identifiers.read().sender_id(addr)
    }

    /// Returns the internal [`SenderId`]s for the given addresses.
    pub fn get_sender_ids(&self, addrs: impl IntoIterator<Item = Address>) -> Vec<SenderId> {
        self.identifiers.write().sender_ids_or_create(addrs)
    }

    /// Returns all senders in the pool
    pub fn unique_senders(&self) -> AddressSet {
        self.get_pool_data().unique_senders()
    }

    /// Converts the changed accounts to a map of sender ids to sender info (internal identifier
    /// used for __tracked__ accounts)
    fn changed_senders(
        &self,
        accs: impl Iterator<Item = ChangedAccount>,
    ) -> FxHashMap<SenderId, SenderInfo> {
        let identifiers = self.identifiers.read();
        accs.into_iter()
            .filter_map(|acc| {
                let ChangedAccount { address, nonce, balance } = acc;
                let sender_id = identifiers.sender_id(&address)?;
                Some((sender_id, SenderInfo { state_nonce: nonce, balance }))
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

        let mut listeners = self.pending_transaction_listener.write();
        // Clean up dead listeners before adding new one
        listeners.retain(|l| !l.sender.is_closed());
        listeners.push(listener);

        rx
    }

    /// Adds a new transaction listener to the pool that gets notified about every new transaction.
    pub fn add_new_transaction_listener(
        &self,
        kind: TransactionListenerKind,
    ) -> mpsc::Receiver<NewTransactionEvent<T::Transaction>> {
        let (sender, rx) = mpsc::channel(self.config.new_tx_listener_buffer_size);
        let listener = TransactionListener { sender, kind };

        let mut listeners = self.transaction_listener.write();
        // Clean up dead listeners before adding new one
        listeners.retain(|l| !l.sender.is_closed());
        listeners.push(listener);

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
        if !self.get_pool_data().contains(&tx_hash) {
            return None
        }
        Some(self.subscribe_transaction_events(tx_hash))
    }

    /// Adds a listener for events of the given transaction without checking that this pool
    /// holds it.
    ///
    /// For [`TransactionPool`](crate::TransactionPool) implementations that keep some
    /// transactions outside this pool and report their events through
    /// [`Self::notify_event_listeners`]. Only subscribe to transactions the caller holds: a
    /// subscription is removed once its hash gets a final event.
    pub fn subscribe_transaction_events(&self, tx_hash: TxHash) -> TransactionEvents {
        let events = self.event_listener.write().subscribe(tx_hash);
        self.mark_event_listener_installed();
        events
    }

    /// Adds a listener for all transaction events.
    pub fn add_all_transactions_event_listener(&self) -> AllTransactionsEvents<T::Transaction> {
        let mut listener = self.event_listener.write();
        let events = listener.subscribe_all();
        self.mark_event_listener_installed();
        events
    }

    #[inline]
    fn has_event_listeners(&self) -> bool {
        self.has_event_listeners.load(Ordering::Relaxed)
    }

    #[inline]
    fn mark_event_listener_installed(&self) {
        self.has_event_listeners.store(true, Ordering::Relaxed);
    }

    #[inline]
    fn update_event_listener_state(&self, listener: &PoolEventBroadcast<T::Transaction>) {
        if listener.is_empty() {
            self.has_event_listeners.store(false, Ordering::Relaxed);
        }
    }

    #[inline]
    fn with_event_listener<F>(&self, emit: F)
    where
        F: FnOnce(&mut PoolEventBroadcast<T::Transaction>),
    {
        if !self.has_event_listeners() {
            return
        }
        let mut listener = self.event_listener.write();
        if !listener.is_empty() {
            emit(&mut listener);
        }
        self.update_event_listener_state(&listener);
    }

    /// Returns a read lock to the pool's data.
    pub fn get_pool_data(&self) -> RwLockReadGuard<'_, TxPool<T>> {
        self.pool.read()
    }

    /// Returns transactions in the pool that can be propagated
    pub fn pooled_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut out = Vec::new();
        self.append_pooled_transactions(&mut out);
        out
    }

    /// Returns hashes of transactions in the pool that can be propagated.
    pub fn pooled_transactions_hashes(&self) -> Vec<TxHash> {
        let mut out = Vec::new();
        self.append_pooled_transactions_hashes(&mut out);
        out
    }

    /// Returns only the first `max` transactions in the pool that can be propagated.
    pub fn pooled_transactions_max(
        &self,
        max: usize,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if max == 0 {
            return Vec::new()
        }

        let pool = self.get_pool_data();
        let mut out = Vec::with_capacity(max.min(pool.all().len()));
        out.extend(pool.all().transactions_iter().filter(|tx| tx.propagate).take(max).cloned());
        out
    }

    /// Extends the given vector with all transactions in the pool that can be propagated.
    pub fn append_pooled_transactions(
        &self,
        out: &mut Vec<Arc<ValidPoolTransaction<T::Transaction>>>,
    ) {
        out.extend(
            self.get_pool_data().all().transactions_iter().filter(|tx| tx.propagate).cloned(),
        );
    }

    /// Extends the given vector with pooled transactions for the given hashes that are allowed to
    /// be propagated.
    pub fn append_pooled_transaction_elements(
        &self,
        tx_hashes: &[TxHash],
        limit: GetPooledTransactionLimit,
        out: &mut Vec<<<V as TransactionValidator>::Transaction as PoolTransaction>::Pooled>,
    ) where
        <V as TransactionValidator>::Transaction: EthPoolTransaction,
    {
        let transactions = self.get_all_propagatable(tx_hashes);
        let mut size = 0;
        for transaction in transactions {
            let encoded_len = transaction.encoded_length();
            let Some(pooled) = self.to_pooled_transaction(transaction) else {
                continue;
            };

            size += encoded_len;
            out.push(pooled.into_inner());

            if limit.exceeds(size) {
                break
            }
        }
    }

    /// Extends the given vector with the hashes of all transactions in the pool that can be
    /// propagated.
    pub fn append_pooled_transactions_hashes(&self, out: &mut Vec<TxHash>) {
        out.extend(
            self.get_pool_data()
                .all()
                .transactions_iter()
                .filter(|tx| tx.propagate)
                .map(|tx| *tx.hash()),
        );
    }

    /// Extends the given vector with only the first `max` transactions in the pool that can be
    /// propagated.
    pub fn append_pooled_transactions_max(
        &self,
        max: usize,
        out: &mut Vec<Arc<ValidPoolTransaction<T::Transaction>>>,
    ) {
        out.extend(
            self.get_pool_data()
                .all()
                .transactions_iter()
                .filter(|tx| tx.propagate)
                .take(max)
                .cloned(),
        );
    }

    /// Returns only the first `max` hashes of transactions in the pool that can be propagated.
    pub fn pooled_transactions_hashes_max(&self, max: usize) -> Vec<TxHash> {
        if max == 0 {
            return Vec::new();
        }

        let pool = self.get_pool_data();
        let mut out = Vec::with_capacity(max.min(pool.all().len()));
        out.extend(
            pool.all().transactions_iter().filter(|tx| tx.propagate).take(max).map(|tx| *tx.hash()),
        );
        out
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
        if transaction.is_blob_transaction() {
            let sidecar = self.blob_store.get(*transaction.hash()).ok()??;
            transaction.transaction.clone().try_into_pooled_blob(sidecar)
        } else {
            transaction
                .transaction
                .clone_into_pooled()
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
        let mut elements = Vec::new();
        self.append_pooled_transaction_elements(&tx_hashes, limit, &mut elements);
        elements.shrink_to_fit();
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
    /// Public frames are withdrawn atomically with advancing the canonical head.
    pub fn on_canonical_state_change(&self, update: CanonicalStateUpdate<'_, V::Block>) {
        trace!(target: "txpool", ?update, "updating pool on canonical state change");

        let block_info = update.block_info();
        let CanonicalStateUpdate {
            new_tip, changed_accounts, mined_transactions, update_kind, ..
        } = update;
        self.validator.on_new_head_block(new_tip);

        let changed_senders = self.changed_senders(changed_accounts.into_iter());

        // update the pool
        let outcome = {
            let mut pool = self.pool.write();
            // Recheck fork-dependent wire validity even when no selected nonce changed.
            let affected = pool.frame_transaction_hashes();
            let mut queue = self.frame_revalidation.lock();
            let mut discarded = Vec::new();
            for hash in &mined_transactions {
                queue.cancel(hash);
            }
            if !affected.is_empty() {
                let mined = mined_transactions.iter().copied().collect::<B256Set>();
                for tx in pool.remove_transactions(affected) {
                    if !mined.contains(tx.hash()) {
                        discarded.extend(queue.insert(tx));
                    }
                }
            }
            discarded.extend(queue.discard_worst(self.frame_revalidation_limit()));
            let mut outcome = pool.on_canonical_state_change(
                block_info,
                mined_transactions,
                changed_senders,
                update_kind,
            );
            outcome.discarded.extend(discarded);
            outcome
        };

        // This will discard outdated transactions based on the account's nonce
        self.delete_discarded_blobs(outcome.discarded.iter());

        // notify listeners about updates
        self.notify_on_new_state(outcome);
    }

    /// Revalidates withdrawn transactions without restoring cancelled attempts.
    ///
    /// Head changes and validation errors retain ownership for a later maintenance pass.
    /// Each candidate is attempted at most once per call.
    pub async fn revalidate_frame_transactions(&self) {
        let candidates = self.frame_revalidation.lock().snapshot();
        for candidate in candidates {
            let validation_head = self.block_info().last_seen_block_hash;
            let validation = self
                .validator
                .validate_transaction(candidate.origin, candidate.transaction.clone())
                .await;
            if let TransactionValidationOutcome::Error(_, err) = &validation {
                debug!(target: "txpool", tx_hash = ?candidate.hash(), %err,
                    "deferring frame revalidation after validation error");
                continue
            }
            let (meta, discarded) = {
                let mut pool = self.pool.write();
                let current_head = pool.block_info().last_seen_block_hash;
                if current_head != validation_head ||
                    validation
                        .as_valid_transaction()
                        .and_then(|tx| tx.transaction().frame_validation())
                        .is_some_and(|metadata| metadata.head_hash != current_head)
                {
                    trace!(target: "txpool", tx_hash = ?candidate.hash(),
                        "deferring frame revalidation until validation and pool heads agree");
                    continue
                }
                // Explicit removal or a newer sender transaction cancels this exact attempt.
                if !self.frame_revalidation.lock().take_current(&candidate) {
                    continue
                }
                let (result, meta) =
                    self.add_transaction(&mut pool, candidate.origin, validation, Instant::now());
                let discarded = match result {
                    Ok(_) => {
                        let discarded = pool.discard_worst();
                        pool.update_size_metrics();
                        discarded
                    }
                    Err(err) => {
                        debug!(target: "txpool", tx_hash = ?candidate.hash(), %err,
                            "discarding frame transaction after revalidation");
                        // Terminal rejection releases the withdrawn transaction's sidecars.
                        vec![Arc::clone(&candidate)]
                    }
                };
                (meta, discarded)
            };
            if let Some(meta) = meta &&
                !discarded.iter().any(|tx| tx.hash() == meta.added.hash())
            {
                self.on_added_transaction(meta);
            }
            if !discarded.is_empty() {
                self.delete_discarded_blobs(discarded.iter());
                self.with_event_listener(|listener| listener.discarded_many(&discarded));
            }
        }
    }

    /// Performs account updates on the pool.
    ///
    /// This will either promote or discard transactions based on the new account state.
    ///
    /// This should be invoked when the pool drifted and accounts are updated manually
    pub fn update_accounts(&self, accounts: Vec<ChangedAccount>) {
        let dependencies = FrameDependencies {
            accounts: accounts.iter().map(|account| account.address).collect(),
            ..Default::default()
        };
        let changed_senders = self.changed_senders(accounts.into_iter());
        let UpdateOutcome { promoted, discarded } = {
            let mut pool = self.pool.write();
            let affected = pool.affected_frame_transactions(&dependencies, 0, None);
            let mut queue = self.frame_revalidation.lock();
            let mut discarded = Vec::new();
            for tx in pool.remove_transactions(affected) {
                discarded.extend(queue.insert(tx));
            }
            discarded.extend(queue.discard_worst(self.frame_revalidation_limit()));
            let mut outcome = pool.update_accounts(changed_senders);
            outcome.discarded.extend(discarded);
            outcome
        };

        self.notify_on_transaction_updates(promoted, discarded);
    }

    /// Add a single validated transaction into the pool.
    ///
    /// Returns the outcome and optionally metadata to be processed after the pool lock is
    /// released.
    ///
    /// Note: this is only used internally by [`Self::add_transactions()`], all new transaction(s)
    /// come in through that function, either as a batch or `std::iter::once`.
    fn add_transaction(
        &self,
        pool: &mut RwLockWriteGuard<'_, TxPool<T>>,
        origin: TransactionOrigin,
        tx: TransactionValidationOutcome<T::Transaction>,
        timestamp: Instant,
    ) -> (PoolResult<AddedTransactionOutcome>, Option<AddedTransactionMeta<T::Transaction>>) {
        match tx {
            TransactionValidationOutcome::Valid {
                balance,
                state_nonce,
                transaction,
                propagate,
                bytecode_hash,
                authorities,
            } => {
                let sender = transaction.sender();
                // split the valid transaction and the blob sidecar if it has any
                let (transaction, blob_sidecar) = match transaction {
                    ValidTransaction::Valid(tx) => (tx, None),
                    ValidTransaction::ValidWithSidecar { transaction, sidecar } => {
                        debug_assert!(
                            transaction.is_blob_transaction(),
                            "validator returned sidecar for a transaction without blobs"
                        );
                        (transaction, Some(sidecar))
                    }
                };

                if transaction.eip8250_nonce_keys().is_some() &&
                    self.frame_revalidation.lock().contains_sender(sender)
                {
                    return (
                        Err(PoolError::other(
                            *transaction.hash(),
                            "sender frame transaction awaits revalidation",
                        )),
                        None,
                    );
                }
                let sender_id = match transaction.eip8250_nonce_keys() {
                    Some(keys) if !keys.is_empty() => pool
                        .all()
                        .keyed_sender_id(sender, keys)
                        .unwrap_or_else(|| self.identifiers.write().next_id()),
                    _ => self.get_sender_id(sender),
                };
                let transaction_id = TransactionId::new(sender_id, transaction.nonce());

                let tx = ValidPoolTransaction {
                    transaction,
                    transaction_id,
                    propagate,
                    timestamp,
                    origin,
                    authority_ids: authorities.map(|auths| self.get_sender_ids(auths)),
                };

                let added = match pool.add_transaction(tx, balance, state_nonce, bytecode_hash) {
                    Ok(added) => added,
                    Err(err) => return (Err(err), None),
                };
                let hash = *added.hash();
                let transaction = match &added {
                    AddedTransaction::Pending(added) => &added.transaction,
                    AddedTransaction::Parked { transaction, .. } => transaction,
                };
                let cancelled_frame = self.frame_revalidation.lock().cancel_replaced(
                    transaction.sender(),
                    transaction.nonce(),
                    transaction.transaction.eip8250_nonce_keys(),
                );
                let state = added.transaction_state();

                let meta = AddedTransactionMeta { added, blob_sidecar, cancelled_frame };

                (Ok(AddedTransactionOutcome { hash, state }), Some(meta))
            }
            TransactionValidationOutcome::Invalid(tx, err) => {
                self.with_event_listener(|listener| listener.invalid(tx.hash()));
                (Err(PoolError::new(*tx.hash(), err)), None)
            }
            TransactionValidationOutcome::Error(tx_hash, err) => {
                self.with_event_listener(|listener| listener.discarded(&tx_hash));
                (Err(PoolError::other(tx_hash, err)), None)
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
            let events = listener.subscribe(tx.tx_hash());
            self.mark_event_listener_installed();
            events
        };
        let mut results = self.add_transactions(origin, std::iter::once(tx));
        results.pop().expect("result length is the same as the input")?;
        Ok(listener)
    }

    /// Adds all transactions in the iterator to the pool, returning a list of results.
    ///
    /// Convenience method that assigns the same origin to all transactions. Delegates to
    /// [`Self::add_transactions_with_origins`].
    pub fn add_transactions(
        &self,
        origin: TransactionOrigin,
        transactions: impl IntoIterator<Item = TransactionValidationOutcome<T::Transaction>>,
    ) -> Vec<PoolResult<AddedTransactionOutcome>> {
        self.add_transactions_with_origins(transactions.into_iter().map(|tx| (origin, tx)))
    }

    /// Adds all transactions in the iterator to the pool, each with its own
    /// [`TransactionOrigin`], returning a list of results.
    pub fn add_transactions_with_origins(
        &self,
        transactions: impl IntoIterator<
            Item = (TransactionOrigin, TransactionValidationOutcome<T::Transaction>),
        >,
    ) -> Vec<PoolResult<AddedTransactionOutcome>> {
        let transactions = transactions.into_iter();
        let mut added_metas = Vec::with_capacity(transactions.size_hint().0);

        // Collect results and metadata while holding the pool write lock
        let (mut results, added_metas, discarded) = {
            let mut pool = self.pool.write();
            let timestamp = Instant::now();

            let results = transactions
                .map(|(origin, tx)| {
                    let (result, meta) = self.add_transaction(&mut pool, origin, tx, timestamp);

                    // Only collect metadata for successful insertions
                    if result.is_ok() &&
                        let Some(meta) = meta
                    {
                        added_metas.push(meta);
                    }

                    result
                })
                .collect::<Vec<_>>();

            // Enforce the pool size limits if at least one transaction was added successfully
            let discarded = if results.iter().any(Result::is_ok) {
                let discarded = pool.discard_worst();
                pool.update_size_metrics();
                discarded
            } else {
                Default::default()
            };

            (results, added_metas, discarded)
        };

        for meta in added_metas {
            self.on_added_transaction(meta);
        }

        if !discarded.is_empty() {
            // Delete any blobs associated with discarded blob transactions
            self.delete_discarded_blobs(discarded.iter());
            self.with_event_listener(|listener| listener.discarded_many(&discarded));

            // Linear search avoids allocating a hash set for small eviction batches.
            const MAX_LINEAR_SEARCH_DISCARDS: usize = 4;
            let discarded_hashes = (discarded.len() > MAX_LINEAR_SEARCH_DISCARDS)
                .then(|| discarded.iter().map(|tx| *tx.hash()).collect::<B256Set>());
            let is_discarded = |hash: &TxHash| match &discarded_hashes {
                Some(hashes) => hashes.contains(hash),
                None => discarded.iter().any(|tx| tx.hash() == hash),
            };

            // A newly added transaction may be immediately discarded, so we need to
            // adjust the result here
            for res in &mut results {
                if let Ok(AddedTransactionOutcome { hash, .. }) = res &&
                    is_discarded(hash)
                {
                    *res = Err(PoolError::new(*hash, PoolErrorKind::DiscardedOnInsert))
                }
            }
        };

        results
    }

    /// Process a transaction that was added to the pool.
    ///
    /// Performs blob storage operations and sends all notifications. This should be called
    /// after the pool write lock has been released to avoid blocking pool operations.
    fn on_added_transaction(&self, mut meta: AddedTransactionMeta<T::Transaction>) {
        if let Some(cancelled) = meta.cancelled_frame.take() &&
            cancelled.hash() != meta.added.hash()
        {
            self.delete_discarded_blobs(std::iter::once(&cancelled));
            self.with_event_listener(|listener| listener.replaced(cancelled, *meta.added.hash()));
        }
        // Handle blob sidecar storage and notifications for EIP-4844 transactions
        if let Some(sidecar) = meta.blob_sidecar {
            let hash = *meta.added.hash();
            self.on_new_blob_sidecar(&hash, &sidecar);
            self.insert_blob(hash, sidecar);
        }

        // Delete replaced blob sidecar if any
        if let Some(replaced) = meta.added.replaced_blob_transaction() {
            debug!(target: "txpool", "[{:?}] delete replaced blob sidecar", replaced);
            self.delete_blob(replaced);
        }

        // Delete discarded blob sidecars if any, this doesnt do any IO.
        if let Some(discarded) = meta.added.discarded_transactions() {
            self.delete_discarded_blobs(discarded.iter());
        }

        // Notify pending transaction listeners
        if let Some(pending) = meta.added.as_pending() {
            self.on_new_pending_transaction(pending);
        }

        // A parked insertion can promote older transactions when validation observes newer
        // account state. Notify those promotions independently of the inserted transaction.
        let promoted = meta.added.take_parked_promoted();
        if !promoted.is_empty() {
            self.notify_on_transaction_updates(promoted, Vec::new());
        }

        // Notify event listeners
        self.notify_event_listeners(&meta.added);

        // Notify new transaction listeners
        self.on_new_transaction(meta.added.into_new_transaction_event());
    }

    /// Notify all listeners about a new pending transaction.
    ///
    /// See also [`Self::add_pending_listener`]
    ///
    /// CAUTION: This function is only intended to be used manually in order to use this type's
    /// pending transaction receivers when manually implementing the
    /// [`TransactionPool`](crate::TransactionPool) trait for a custom pool implementation
    /// [`TransactionPool::pending_transactions_listener_for`](crate::TransactionPool).
    pub fn on_new_pending_transaction(&self, pending: &AddedPendingTransaction<T::Transaction>) {
        let mut needs_cleanup = false;

        {
            let listeners = self.pending_transaction_listener.read();
            for listener in listeners.iter() {
                if !listener.send_all(pending.pending_transactions(listener.kind)) {
                    needs_cleanup = true;
                }
            }
        }

        // Clean up dead listeners if we detected any closed channels
        if needs_cleanup {
            self.pending_transaction_listener
                .write()
                .retain(|listener| !listener.sender.is_closed());
        }
    }

    /// Notify all listeners about a newly inserted pending transaction.
    ///
    /// See also [`Self::add_new_transaction_listener`]
    ///
    /// CAUTION: This function is only intended to be used manually in order to use this type's
    /// transaction receivers when manually implementing the
    /// [`TransactionPool`](crate::TransactionPool) trait for a custom pool implementation
    /// [`TransactionPool::new_transactions_listener_for`](crate::TransactionPool).
    pub fn on_new_transaction(&self, event: NewTransactionEvent<T::Transaction>) {
        let mut needs_cleanup = false;

        {
            let listeners = self.transaction_listener.read();
            for listener in listeners.iter() {
                if listener.kind.is_propagate_only() && !event.transaction.propagate {
                    if listener.sender.is_closed() {
                        needs_cleanup = true;
                    }
                    // Skip non-propagate transactions for propagate-only listeners
                    continue
                }

                if !listener.send(event.clone()) {
                    needs_cleanup = true;
                }
            }
        }

        // Clean up dead listeners if we detected any closed channels
        if needs_cleanup {
            self.transaction_listener.write().retain(|listener| !listener.sender.is_closed());
        }
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

        // notify about promoted pending transactions - emit hashes
        let mut needs_pending_cleanup = false;
        {
            let listeners = self.pending_transaction_listener.read();
            for listener in listeners.iter() {
                if !listener.send_all(outcome.pending_transactions(listener.kind)) {
                    needs_pending_cleanup = true;
                }
            }
        }
        if needs_pending_cleanup {
            self.pending_transaction_listener.write().retain(|l| !l.sender.is_closed());
        }

        // emit full transactions
        let mut needs_tx_cleanup = false;
        {
            let listeners = self.transaction_listener.read();
            for listener in listeners.iter() {
                if !listener.send_all(outcome.full_pending_transactions(listener.kind)) {
                    needs_tx_cleanup = true;
                }
            }
        }
        if needs_tx_cleanup {
            self.transaction_listener.write().retain(|l| !l.sender.is_closed());
        }

        let OnNewCanonicalStateOutcome { mined, promoted, discarded, block_hash } = outcome;

        // broadcast specific transaction events
        self.with_event_listener(|listener| {
            for tx in &mined {
                listener.mined(tx, block_hash);
            }
            for tx in &promoted {
                listener.pending(tx.hash(), None);
            }
            for tx in &discarded {
                listener.discarded(tx.hash());
            }
        })
    }

    /// Notifies all listeners about the transaction movements.
    ///
    /// This will emit events according to the provided changes.
    ///
    /// CAUTION: This function is only intended to be used manually in order to use this type's
    /// [`TransactionEvents`] receivers when manually implementing the
    /// [`TransactionPool`](crate::TransactionPool) trait for a custom pool implementation
    /// [`TransactionPool::transaction_event_listener`](crate::TransactionPool).
    pub fn notify_on_transaction_updates(
        &self,
        promoted: Vec<Arc<ValidPoolTransaction<T::Transaction>>>,
        discarded: Vec<Arc<ValidPoolTransaction<T::Transaction>>>,
    ) {
        // Notify about promoted pending transactions (similar to notify_on_new_state)
        if !promoted.is_empty() {
            let mut needs_pending_cleanup = false;
            {
                let listeners = self.pending_transaction_listener.read();
                for listener in listeners.iter() {
                    let promoted_hashes = promoted.iter().filter_map(|tx| {
                        if listener.kind.is_propagate_only() && !tx.propagate {
                            None
                        } else {
                            Some(*tx.hash())
                        }
                    });
                    if !listener.send_all(promoted_hashes) {
                        needs_pending_cleanup = true;
                    }
                }
            }
            if needs_pending_cleanup {
                self.pending_transaction_listener.write().retain(|l| !l.sender.is_closed());
            }

            // in this case we should also emit promoted transactions in full
            let mut needs_tx_cleanup = false;
            {
                let listeners = self.transaction_listener.read();
                for listener in listeners.iter() {
                    let promoted_txs = promoted.iter().filter_map(|tx| {
                        if listener.kind.is_propagate_only() && !tx.propagate {
                            None
                        } else {
                            Some(NewTransactionEvent::pending(tx.clone()))
                        }
                    });
                    if !listener.send_all(promoted_txs) {
                        needs_tx_cleanup = true;
                    }
                }
            }
            if needs_tx_cleanup {
                self.transaction_listener.write().retain(|l| !l.sender.is_closed());
            }
        }

        self.with_event_listener(|listener| {
            for tx in &promoted {
                listener.pending(tx.hash(), None);
            }
            for tx in &discarded {
                listener.discarded(tx.hash());
            }
        });

        if !discarded.is_empty() {
            // This deletes outdated blob txs from the blob store, based on the account's nonce.
            // This is called during txpool maintenance when the pool drifted.
            self.delete_discarded_blobs(discarded.iter());
        }
    }

    /// Fire events for the newly added transaction if there are any.
    ///
    /// See also [`Self::add_transaction_event_listener`].
    ///
    /// CAUTION: This function is only intended to be used manually in order to use this type's
    /// [`TransactionEvents`] receivers when manually implementing the
    /// [`TransactionPool`](crate::TransactionPool) trait for a custom pool implementation
    /// [`TransactionPool::transaction_event_listener`](crate::TransactionPool).
    pub fn notify_event_listeners(&self, tx: &AddedTransaction<T::Transaction>) {
        self.with_event_listener(|listener| match tx {
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
            AddedTransaction::Parked { transaction, replaced, queued_reason, .. } => {
                listener.queued(transaction.hash(), queued_reason.clone());
                if let Some(replaced) = replaced {
                    listener.replaced(replaced.clone(), *transaction.hash());
                }
            }
        });
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

    /// Returns all transactions of the given sender, collected under a single read guard so that
    /// both sides reflect the same pool state.
    pub fn all_transactions_by_sender(
        &self,
        sender: Address,
    ) -> AllPoolTransactions<T::Transaction> {
        let pool = self.get_pool_data();
        AllPoolTransactions {
            pending: pool
                .all()
                .sender_lanes(sender)
                .flat_map(|id| pool.pending_txs_by_sender(id))
                .collect(),
            queued: pool
                .all()
                .sender_lanes(sender)
                .flat_map(|id| pool.queued_txs_by_sender(id))
                .collect(),
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
        let removed = {
            let mut pool = self.pool.write();
            let mut queue = self.frame_revalidation.lock();
            let mut removed = Vec::new();
            for hash in &hashes {
                removed.extend(queue.remove(hash));
            }
            removed.extend(pool.remove_transactions(hashes));
            removed
        };

        self.with_event_listener(|listener| listener.discarded_many(&removed));

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
        let removed = {
            let mut pool = self.pool.write();
            let mut queue = self.frame_revalidation.lock();
            let mut cancelled = Vec::new();
            for hash in &hashes {
                if let Some(tx) = queue.remove(hash) {
                    pool.remove_descendants(tx.id(), &mut cancelled);
                    cancelled.push(tx);
                }
            }
            let mut removed = pool.remove_transactions_and_descendants(hashes);
            for tx in &removed {
                cancelled.extend(queue.cancel_sender(tx.sender()));
            }
            removed.extend(cancelled);
            removed
        };

        self.with_event_listener(|listener| {
            for tx in &removed {
                listener.discarded(tx.hash());
            }
        });

        removed
    }

    /// Removes and returns all transactions by the specified sender from the pool.
    pub fn remove_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let removed = {
            let mut pool = self.pool.write();
            let lanes = pool.all().sender_lanes(sender).collect::<Vec<_>>();
            let mut removed: Vec<_> =
                lanes.into_iter().flat_map(|id| pool.remove_transactions_by_sender(id)).collect();
            removed.extend(self.frame_revalidation.lock().cancel_sender(sender));
            removed
        };

        self.with_event_listener(|listener| listener.discarded_many(&removed));

        removed
    }

    /// Prunes and returns all matching transactions from the pool.
    ///
    /// This removes the transactions as if they were mined: descendant transactions are **not**
    /// parked and remain eligible for inclusion.
    pub fn prune_transactions(
        &self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if hashes.is_empty() {
            return Vec::new()
        }

        let mut pool = self.pool.write();
        let mut queue = self.frame_revalidation.lock();
        for hash in &hashes {
            queue.cancel(hash);
        }
        pool.prune_transactions(hashes)
    }

    /// Retains only transactions that are not present in the pool.
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

    /// Retains only transactions that are present in the pool.
    pub fn retain_contains<A>(&self, announcement: &mut A)
    where
        A: HandleMempoolData,
    {
        if announcement.is_empty() {
            return
        }
        let pool = self.get_pool_data();
        announcement.retain_by_hash(|tx| pool.contains(tx))
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
        let pool = self.get_pool_data();
        pool.all().sender_lanes(sender).flat_map(|id| pool.get_transactions_by_sender(id)).collect()
    }

    /// Returns a pending transaction sent by the given sender with the given nonce.
    pub fn get_pending_transaction_by_sender_and_nonce(
        &self,
        sender: Address,
        nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.sender_id(&sender)?;
        self.get_pool_data().get_pending_transaction_by_sender_and_nonce(sender_id, nonce)
    }

    /// Returns all queued transactions of the address by sender
    pub fn get_queued_transactions_by_sender(
        &self,
        sender: Address,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let pool = self.get_pool_data();
        pool.all().sender_lanes(sender).flat_map(|id| pool.queued_txs_by_sender(id)).collect()
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
        let pool = self.get_pool_data();
        pool.all().sender_lanes(sender).flat_map(|id| pool.pending_txs_by_sender(id)).collect()
    }

    /// Returns all pending transactions for a resolved sender ID.
    pub fn get_pending_transactions_by_sender_id(
        &self,
        sender_id: SenderId,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().pending_txs_by_sender(sender_id)
    }

    /// Returns the highest transaction of the address
    pub fn get_highest_transaction_by_sender(
        &self,
        sender: Address,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.sender_id(&sender)?;
        self.get_highest_transaction_by_sender_id(sender_id)
    }

    /// Returns the highest transaction for a resolved sender ID.
    pub fn get_highest_transaction_by_sender_id(
        &self,
        sender_id: SenderId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.get_pool_data().get_highest_transaction_by_sender(sender_id)
    }

    /// Returns the transaction with the highest nonce that is executable given the on chain nonce.
    pub fn get_highest_consecutive_transaction_by_sender(
        &self,
        sender: Address,
        on_chain_nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let sender_id = self.sender_id(&sender)?;
        self.get_highest_consecutive_transaction_by_sender_id(sender_id, on_chain_nonce)
    }

    /// Returns the highest consecutive transaction for a resolved sender ID and on-chain nonce.
    pub fn get_highest_consecutive_transaction_by_sender_id(
        &self,
        sender_id: SenderId,
        on_chain_nonce: u64,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
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
        txs: &[TxHash],
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        if txs.is_empty() {
            return Vec::new()
        }
        let pool = self.get_pool_data();
        txs.iter().filter_map(|tx| pool.get(tx).filter(|tx| tx.propagate)).collect()
    }

    /// Notify about propagated transactions.
    pub fn on_propagated(&self, txs: PropagatedTransactions) {
        if txs.is_empty() {
            return
        }
        self.with_event_listener(|listener| {
            txs.into_iter().for_each(|(hash, peers)| listener.propagated(&hash, peers));
        });
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
    fn insert_blob(&self, hash: TxHash, blob: PooledBlobSidecar) {
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

    /// Bounds withdrawn ownership by the pool's existing aggregate capacity.
    fn frame_revalidation_limit(&self) -> SubPoolLimit {
        [
            self.config.pending_limit,
            self.config.basefee_limit,
            self.config.queued_limit,
            self.config.blob_limit,
        ]
        .into_iter()
        .fold(SubPoolLimit::new(0, 0), |total, limit| {
            SubPoolLimit::new(
                total.max_txs.saturating_add(limit.max_txs),
                total.max_size.saturating_add(limit.max_size),
            )
        })
    }

    /// Deletes all blob transactions that were discarded.
    fn delete_discarded_blobs<'a>(
        &'a self,
        transactions: impl IntoIterator<Item = &'a Arc<ValidPoolTransaction<T::Transaction>>>,
    ) {
        let blob_txs = transactions
            .into_iter()
            .filter(|tx| tx.transaction.is_blob_transaction())
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

/// Metadata for a transaction that was added to the pool.
///
/// This holds all the data needed to complete post-insertion operations (notifications,
/// blob storage).
#[derive(Debug)]
struct AddedTransactionMeta<T: PoolTransaction> {
    /// The transaction that was added to the pool
    added: AddedTransaction<T>,
    /// Optional blob sidecar for EIP-4844 transactions
    blob_sidecar: Option<PooledBlobSidecar>,
    /// An in-flight revalidation superseded by this newly admitted sender transaction.
    cancelled_frame: Option<Arc<ValidPoolTransaction<T>>>,
}

/// Tracks an added transaction and all graph changes caused by adding it.
#[derive(Debug, Clone)]
pub struct AddedPendingTransaction<T: PoolTransaction> {
    /// Inserted transaction.
    pub transaction: Arc<ValidPoolTransaction<T>>,
    /// Replaced transaction.
    pub replaced: Option<Arc<ValidPoolTransaction<T>>>,
    /// transactions promoted to the pending queue
    pub promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// transactions that failed and became discarded
    pub discarded: Vec<Arc<ValidPoolTransaction<T>>>,
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
        /// The specific reason why the transaction is queued (if applicable).
        queued_reason: Option<QueuedReason>,
        /// Existing transactions promoted to pending by this insertion.
        ///
        /// Validation can observe newer account state before pool maintenance applies it.
        /// Insertion rechecks the sender's transactions against that state, so lower-nonce
        /// transactions can become pending while the inserted transaction remains parked.
        promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    },
}

impl<T: PoolTransaction> AddedTransaction<T> {
    /// Returns whether the transaction has been added to the pending pool.
    pub const fn as_pending(&self) -> Option<&AddedPendingTransaction<T>> {
        match self {
            Self::Pending(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the replaced transaction if there was one
    pub const fn replaced(&self) -> Option<&Arc<ValidPoolTransaction<T>>> {
        match self {
            Self::Pending(tx) => tx.replaced.as_ref(),
            Self::Parked { replaced, .. } => replaced.as_ref(),
        }
    }

    /// Takes promotions caused by a parked insertion for separate notification.
    ///
    /// Call before notifying listeners about the inserted transaction. This drains only the
    /// parked promotions; subsequent parked event conversion handles the inserted transaction.
    pub(crate) fn take_parked_promoted(&mut self) -> Vec<Arc<ValidPoolTransaction<T>>> {
        match self {
            Self::Parked { promoted, .. } => std::mem::take(promoted),
            Self::Pending(_) => Vec::new(),
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
        self.replaced()
            .filter(|tx| tx.transaction.is_blob_transaction())
            .map(|tx| *tx.transaction.hash())
    }

    /// Returns the hash of the transaction
    pub fn hash(&self) -> &TxHash {
        match self {
            Self::Pending(tx) => tx.transaction.hash(),
            Self::Parked { transaction, .. } => transaction.hash(),
        }
    }

    /// Converts this type into the event type for listeners
    pub fn into_new_transaction_event(self) -> NewTransactionEvent<T> {
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

    /// Returns the queued reason if the transaction is parked with a queued reason.
    pub const fn queued_reason(&self) -> Option<&QueuedReason> {
        match self {
            Self::Pending(_) => None,
            Self::Parked { queued_reason, .. } => queued_reason.as_ref(),
        }
    }

    /// Returns the transaction state based on the subpool and queued reason.
    pub fn transaction_state(&self) -> AddedTransactionState {
        match self.subpool() {
            SubPool::Pending => AddedTransactionState::Pending,
            _ => {
                // For non-pending transactions, use the queued reason directly from the
                // AddedTransaction
                if let Some(reason) = self.queued_reason() {
                    AddedTransactionState::Queued(reason.clone())
                } else {
                    // Fallback - this shouldn't happen with the new implementation
                    AddedTransactionState::Queued(QueuedReason::NonceGap)
                }
            }
        }
    }
}

/// The specific reason why a transaction is queued (not ready for execution)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuedReason {
    /// Transaction has a nonce gap - missing prior transactions
    NonceGap,
    /// Transaction has parked ancestors - waiting for other transactions to be mined
    ParkedAncestors,
    /// Sender has insufficient balance to cover the transaction cost
    InsufficientBalance,
    /// Transaction exceeds the block gas limit
    TooMuchGas,
    /// Transaction doesn't meet the base fee requirement
    InsufficientBaseFee,
    /// Transaction doesn't meet the blob fee requirement (EIP-4844)
    InsufficientBlobFee,
}

/// The state of a transaction when is was added to the pool
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddedTransactionState {
    /// Ready for execution
    Pending,
    /// Not ready for execution due to a specific condition
    Queued(QueuedReason),
}

impl AddedTransactionState {
    /// Returns whether the transaction was submitted as queued.
    pub const fn is_queued(&self) -> bool {
        matches!(self, Self::Queued(_))
    }

    /// Returns whether the transaction was submitted as pending.
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    /// Returns the specific queued reason if the transaction is queued.
    pub const fn queued_reason(&self) -> Option<&QueuedReason> {
        match self {
            Self::Queued(reason) => Some(reason),
            Self::Pending => None,
        }
    }
}

/// The outcome of a successful transaction addition
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedTransactionOutcome {
    /// The hash of the transaction
    pub hash: TxHash,
    /// The state of the transaction
    pub state: AddedTransactionState,
}

impl AddedTransactionOutcome {
    /// Returns whether the transaction was submitted as queued.
    pub const fn is_queued(&self) -> bool {
        self.state.is_queued()
    }

    /// Returns whether the transaction was submitted as pending.
    pub const fn is_pending(&self) -> bool {
        self.state.is_pending()
    }
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
    use super::*;
    use crate::{
        blobstore::{BlobStore, InMemoryBlobStore, PooledBlobSidecar},
        error::InvalidPoolTransactionError,
        identifier::{SenderId, TransactionId},
        test_utils::{
            testing_pool, MockTransaction, MockTransactionFactory, TestPool, TestPoolBuilder,
        },
        validate::{FrameValidation, FrameValidationError, ValidTransaction},
        BlockInfo, CoinbaseTipOrdering, EthPooledTransaction, FullTransactionEvent, PoolConfig,
        PoolUpdateKind, SubPool, SubPoolLimit, TransactionEvent, TransactionListenerKind,
        TransactionOrigin, TransactionPool, TransactionPoolExt, TransactionValidationOutcome,
        ValidPoolTransaction, U256,
    };
    use alloy_consensus::{Transaction, TxEip8141};
    use alloy_eips::{
        eip4844::{Blob, BlobTransactionSidecar, Bytes48},
        eip7594::BlobTransactionSidecarVariant,
        eip8141::{Frame, TransactionFees},
    };
    use alloy_primitives::{Address, Sealable, B256};
    use futures_util::{FutureExt, StreamExt};
    use reth_ethereum_primitives::TransactionSigned;
    use reth_primitives_traits::SealedBlock;
    use std::{collections::VecDeque, sync::Arc};
    use tokio::sync::{mpsc::error::TryRecvError, oneshot};

    /// Inserts a transaction with an explicit validator snapshot, without running maintenance.
    fn insert_with_state_nonce(pool: &TestPool, nonce: u64, state_nonce: u64, propagate: bool) {
        let transaction = MockTransaction::eip1559()
            .with_sender(Address::with_last_byte(1))
            .with_nonce(nonce)
            .with_hash(B256::repeat_byte(nonce as u8));
        pool.pool
            .add_transactions(
                TransactionOrigin::External,
                [TransactionValidationOutcome::Valid {
                    balance: U256::MAX,
                    state_nonce,
                    bytecode_hash: None,
                    transaction: ValidTransaction::Valid(transaction),
                    propagate,
                    authorities: None,
                }],
            )
            .pop()
            .unwrap()
            .unwrap();
    }

    fn transaction_nonces(txs: &[Arc<ValidPoolTransaction<MockTransaction>>]) -> Vec<u64> {
        let mut nonces: Vec<_> = txs.iter().map(|tx| tx.transaction.nonce()).collect();
        nonces.sort_unstable();
        nonces
    }

    #[tokio::test]
    async fn all_transactions_by_sender_across_reclassification() {
        let pool = testing_pool();
        let sender = Address::with_last_byte(1);
        // nonces 0 and 1 are pending, the gapped nonce 9 is queued
        for nonce in [0, 1, 9] {
            let tx =
                MockTransaction::legacy().with_sender(sender).with_nonce(nonce).with_gas_price(100);
            pool.add_transaction(TransactionOrigin::External, tx).await.unwrap();
        }
        let nonces = |txs: &[Arc<ValidPoolTransaction<MockTransaction>>]| {
            let mut nonces: Vec<_> = txs.iter().map(|tx| tx.transaction.nonce()).collect();
            nonces.sort_unstable();
            nonces
        };

        let txs = pool.all_transactions_by_sender(sender);
        assert_eq!(nonces(&txs.pending), [0, 1]);
        assert_eq!(nonces(&txs.queued), [9]);

        let sender_id = pool.inner().sender_id(&sender).unwrap();
        assert_eq!(nonces(&pool.inner().get_pending_transactions_by_sender_id(sender_id)), [0, 1]);
        assert_eq!(
            pool.inner().get_highest_transaction_by_sender_id(sender_id).unwrap().nonce(),
            9
        );
        assert_eq!(
            pool.inner()
                .get_highest_consecutive_transaction_by_sender_id(sender_id, 0)
                .unwrap()
                .nonce(),
            1
        );

        // a higher base fee reclassifies the pending transactions as queued; the snapshot must
        // report each of them on exactly one side
        pool.set_block_info(BlockInfo {
            pending_basefee: 200,
            block_gas_limit: 30_000_000,
            ..Default::default()
        });
        let txs = pool.all_transactions_by_sender(sender);
        assert!(txs.pending.is_empty());
        assert_eq!(nonces(&txs.queued), [0, 1, 9]);
        assert_eq!(nonces(&pool.get_transactions_by_sender(sender)), [0, 1, 9]);
    }

    #[test]
    fn subscribe_transaction_events_without_pool_residency() {
        let test_pool = testing_pool();
        let pool = &test_pool.pool;
        let tx = MockTransactionFactory::default().validated_arc(MockTransaction::eip1559());
        assert!(pool.add_transaction_event_listener(*tx.hash()).is_none());

        let mut events = pool.subscribe_transaction_events(*tx.hash());
        pool.notify_event_listeners(&AddedTransaction::Pending(AddedPendingTransaction {
            transaction: tx,
            replaced: None,
            promoted: vec![],
            discarded: vec![],
        }));
        assert_eq!(events.next().now_or_never(), Some(Some(TransactionEvent::Pending)));
    }

    #[test]
    fn queued_insertion_notifies_older_promotions() {
        let test_pool = testing_pool();
        let pool = &test_pool.pool;
        let mut network = pool.add_pending_listener(TransactionListenerKind::PropagateOnly);
        let mut all = pool.add_pending_listener(TransactionListenerKind::All);
        // Missing nonce 0 keeps both transactions queued.
        insert_with_state_nonce(&test_pool, 1, 0, true);
        insert_with_state_nonce(&test_pool, 2, 0, true);
        let txs = test_pool.all_transactions_by_sender(Address::with_last_byte(1));
        assert!(txs.pending.is_empty());
        assert_eq!(transaction_nonces(&txs.queued), [1, 2]);
        assert_eq!(network.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(all.try_recv(), Err(TryRecvError::Empty));

        let mut full_network =
            pool.add_new_transaction_listener(TransactionListenerKind::PropagateOnly);
        let mut full_all = pool.add_new_transaction_listener(TransactionListenerKind::All);
        let mut events = pool.add_all_transactions_event_listener();

        // A newer validation result says nonce 0 is mined. Nonces 1 and 2 become
        // pending, but inserting 4 leaves a gap at 3.
        insert_with_state_nonce(&test_pool, 4, 1, true);
        let txs = test_pool.all_transactions_by_sender(Address::with_last_byte(1));
        assert_eq!(transaction_nonces(&txs.pending), [1, 2]);
        assert_eq!(transaction_nonces(&txs.queued), [4]);

        let expected = vec![B256::repeat_byte(1), B256::repeat_byte(2)];
        for (kind, listener) in [("network", &mut network), ("all", &mut all)] {
            let mut received = Vec::new();
            while let Ok(hash) = listener.try_recv() {
                received.push(hash);
            }
            received.sort_unstable();
            assert_eq!(received, expected, "{kind} pending notifications");
        }

        // Full-transaction subscribers get each promotion once and the inserted
        // transaction once, still marked queued. Propagation filtering applies here too.
        for (kind, listener) in [("network", &mut full_network), ("all", &mut full_all)] {
            let mut received = Vec::new();
            while let Ok(event) = listener.try_recv() {
                received.push((*event.transaction.hash(), event.subpool));
            }
            received.sort_unstable_by_key(|(hash, _)| *hash);
            let expected = vec![
                (B256::repeat_byte(1), SubPool::Pending),
                (B256::repeat_byte(2), SubPool::Pending),
                (B256::repeat_byte(4), SubPool::Queued),
            ];
            assert_eq!(received, expected, "{kind} full transaction notifications");
        }

        let mut pending_events = Vec::new();
        let mut queued_events = Vec::new();
        while let Some(Some(event)) = events.next().now_or_never() {
            match event {
                FullTransactionEvent::Pending(hash) => pending_events.push(hash),
                FullTransactionEvent::Queued(hash, _) => queued_events.push(hash),
                other => panic!("unexpected event: {other:?}"),
            }
        }
        pending_events.sort_unstable();
        assert_eq!(pending_events, expected);
        assert_eq!(queued_events, vec![B256::repeat_byte(4)]);

        // A later insertion observing the same state must not notify those promotions again.
        insert_with_state_nonce(&test_pool, 5, 1, true);
        assert_eq!(network.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(all.try_recv(), Err(TryRecvError::Empty));
        for listener in [&mut full_network, &mut full_all] {
            let event = listener.try_recv().unwrap();
            assert_eq!(*event.transaction.hash(), B256::repeat_byte(5));
            assert_eq!(event.subpool, SubPool::Queued);
            assert!(matches!(listener.try_recv(), Err(TryRecvError::Empty)));
        }
        assert!(matches!(
            events.next().now_or_never(),
            Some(Some(FullTransactionEvent::Queued(hash, _))) if hash == B256::repeat_byte(5)
        ));
        assert!(events.next().now_or_never().is_none());
    }

    #[test]
    fn queued_insertion_promotions_respect_propagation_filter() {
        let test_pool = testing_pool();
        let pool = &test_pool.pool;
        insert_with_state_nonce(&test_pool, 1, 0, false);
        insert_with_state_nonce(&test_pool, 2, 0, true);

        let mut network = pool.add_pending_listener(TransactionListenerKind::PropagateOnly);
        let mut all = pool.add_pending_listener(TransactionListenerKind::All);
        let mut full_network =
            pool.add_new_transaction_listener(TransactionListenerKind::PropagateOnly);
        let mut full_all = pool.add_new_transaction_listener(TransactionListenerKind::All);
        insert_with_state_nonce(&test_pool, 4, 1, true);

        assert_eq!(network.try_recv().unwrap(), B256::repeat_byte(2));
        assert_eq!(network.try_recv(), Err(TryRecvError::Empty));
        let mut received = Vec::new();
        while let Ok(hash) = all.try_recv() {
            received.push(hash);
        }
        received.sort_unstable();
        assert_eq!(received, [B256::repeat_byte(1), B256::repeat_byte(2)]);

        for (listener, expected) in [
            (
                &mut full_network,
                vec![
                    (B256::repeat_byte(2), SubPool::Pending),
                    (B256::repeat_byte(4), SubPool::Queued),
                ],
            ),
            (
                &mut full_all,
                vec![
                    (B256::repeat_byte(1), SubPool::Pending),
                    (B256::repeat_byte(2), SubPool::Pending),
                    (B256::repeat_byte(4), SubPool::Queued),
                ],
            ),
        ] {
            let mut received = Vec::new();
            while let Ok(event) = listener.try_recv() {
                received.push((*event.transaction.hash(), event.subpool));
            }
            received.sort_unstable_by_key(|(hash, _)| *hash);
            assert_eq!(received, expected);
        }
    }

    #[test]
    fn pending_insertion_notifies_older_promotions() {
        let test_pool = testing_pool();
        let pool = &test_pool.pool;
        insert_with_state_nonce(&test_pool, 1, 0, true);
        insert_with_state_nonce(&test_pool, 2, 0, true);
        let mut network = pool.add_pending_listener(TransactionListenerKind::PropagateOnly);
        let mut all = pool.add_pending_listener(TransactionListenerKind::All);

        // Filling the gap makes both the older transactions and the insertion pending.
        insert_with_state_nonce(&test_pool, 3, 1, true);
        let txs = test_pool.all_transactions_by_sender(Address::with_last_byte(1));
        assert_eq!(transaction_nonces(&txs.pending), [1, 2, 3]);
        assert!(txs.queued.is_empty());
        for listener in [&mut network, &mut all] {
            let mut received = Vec::new();
            while let Ok(hash) = listener.try_recv() {
                received.push(hash);
            }
            received.sort_unstable();
            assert_eq!(
                received,
                [B256::repeat_byte(1), B256::repeat_byte(2), B256::repeat_byte(3)]
            );
        }
    }

    #[test]
    fn test_discard_blobs_on_blob_tx_eviction() {
        // A zero blob, whose commitment and proof are the point at infinity, is a valid sidecar
        // that doesn't need the KZG trusted setup.
        let mut point_at_infinity = [0; 48];
        point_at_infinity[0] = 0xc0;
        let sidecar = BlobTransactionSidecarVariant::Eip4844(BlobTransactionSidecar::new(
            vec![Blob::default()],
            vec![Bytes48::from(point_at_infinity)],
            vec![Bytes48::from(point_at_infinity)],
        ));

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
                blob_store.insert(*tx.get_hash(), sidecar.clone().into()).unwrap();
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
                        sidecar: PooledBlobSidecar::from(sidecar.clone()),
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

        let auth = Address::repeat_byte(1);
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

    #[test]
    fn sender_queries_do_not_allocate_ids_for_unknown_addresses() {
        let test_pool = &TestPoolBuilder::default().with_config(Default::default()).pool;
        let sender = Address::repeat_byte(9);

        assert_eq!(test_pool.sender_id(&sender), None);
        assert!(test_pool.get_transactions_by_sender(sender).is_empty());
        assert!(test_pool.get_pending_transaction_by_sender_and_nonce(sender, 0).is_none());
        assert!(test_pool.get_queued_transactions_by_sender(sender).is_empty());
        assert!(test_pool.get_pending_transactions_by_sender(sender).is_empty());
        let transactions = test_pool.all_transactions_by_sender(sender);
        assert!(transactions.pending.is_empty());
        assert!(transactions.queued.is_empty());
        assert!(test_pool.get_highest_transaction_by_sender(sender).is_none());
        assert!(test_pool.get_highest_consecutive_transaction_by_sender(sender, 0).is_none());
        assert!(test_pool.remove_transactions_by_sender(sender).is_empty());
        assert_eq!(test_pool.sender_id(&sender), None);
    }

    #[test]
    fn all_sender_transactions_include_every_nonce_lane() {
        let pool = &TestPoolBuilder::default().with_config(Default::default()).pool;
        let sender = Address::repeat_byte(7);
        let (ordinary, keyed) = {
            let mut identifiers = pool.identifiers.write();
            (identifiers.sender_id_or_create(sender), identifiers.next_id())
        };
        let mut factory = MockTransactionFactory::default();
        for (lane, nonce, hash) in [(ordinary, 0, 1), (keyed, 0, 2), (keyed, 2, 3)] {
            let mut transaction = factory.validated(
                MockTransaction::legacy()
                    .with_sender(sender)
                    .with_nonce(nonce)
                    .with_hash(B256::repeat_byte(hash))
                    .with_gas_price(10),
            );
            transaction.transaction_id = TransactionId::new(lane, nonce);
            pool.pool.write().add_transaction(transaction, U256::MAX, 0, None).unwrap();
        }
        let transactions = pool.all_transactions_by_sender(sender);
        assert_eq!(transactions.pending.len(), 2);
        assert_eq!(transactions.queued.len(), 1);
        assert_eq!(*transactions.queued[0].hash(), B256::repeat_byte(3));
        assert!(transactions.pending.iter().any(|tx| *tx.hash() == B256::repeat_byte(1)));
        assert!(transactions.pending.iter().any(|tx| *tx.hash() == B256::repeat_byte(2)));
        assert_eq!(pool.get_transactions_by_sender(sender).len(), 3);
        assert_eq!(pool.get_pending_transactions_by_sender(sender).len(), 2);
        assert_eq!(pool.get_queued_transactions_by_sender(sender).len(), 1);

        assert_eq!(
            pool.remove_transactions(vec![B256::repeat_byte(2), B256::repeat_byte(3)]).len(),
            2
        );
        let remaining = pool.get_transactions_by_sender(sender);
        assert_eq!(remaining.len(), 1);
        assert_eq!(*remaining[0].hash(), B256::repeat_byte(1));
        assert_eq!(pool.remove_transactions_by_sender(sender).len(), 1);
        assert!(pool.get_transactions_by_sender(sender).is_empty());
        assert!(pool.pool.read().all().sender_lanes(sender).next().is_none());
        assert_eq!(pool.sender_id(&sender), Some(ordinary));
    }

    #[derive(Debug, Default)]
    struct FrameRevalidationValidator {
        outcomes: Mutex<VecDeque<TransactionValidationOutcome<EthPooledTransaction>>>,
        gate: Mutex<Option<oneshot::Receiver<()>>>,
    }

    impl TransactionValidator for FrameRevalidationValidator {
        type Transaction = EthPooledTransaction;
        type Block = reth_ethereum_primitives::Block;

        async fn validate_transaction(
            &self,
            _origin: TransactionOrigin,
            _transaction: Self::Transaction,
        ) -> TransactionValidationOutcome<Self::Transaction> {
            let gate = self.gate.lock().take();
            if let Some(gate) = gate {
                gate.await.unwrap();
            }
            self.outcomes.lock().pop_front().expect("one scripted validation per maintenance pass")
        }
    }

    fn frame_revalidation_transaction(
        head_hash: B256,
        fee_per_gas: u64,
        sender: Address,
        nonce: u64,
    ) -> EthPooledTransaction {
        let mut tx = TxEip8141 {
            chain_id: 1,
            sender,
            nonce,
            nonce_keys: None,
            frames: vec![Frame::default()],
            fees: TransactionFees::default(),
            blob_versioned_hashes: vec![B256::repeat_byte(1)],
            ..Default::default()
        };
        tx.fees.max_fee = U256::from(fee_per_gas) * U256::from(tx.gas_limit());
        let mut transaction = EthPooledTransaction::new(
            Recovered::new_unchecked(TransactionSigned::Eip8141(tx.seal_slow()), sender),
            0,
        );
        transaction
            .set_frame_validation(Arc::new(FrameValidation {
                sender,
                sender_nonce: nonce,
                nonce_keys: None,
                state_nonce: nonce,
                sender_balance: U256::MAX,
                sender_code_hash: None,
                payer: sender,
                max_cost: U256::from(1_000_000),
                payer_balance: U256::MAX,
                head_hash,
                dependencies: Default::default(),
                expires_at: None,
                recent_root_dependencies: vec![],
                exclusive_payer: false,
            }))
            .unwrap();
        transaction
    }

    fn frame_revalidation_outcome(
        mut transaction: EthPooledTransaction,
        head_hash: B256,
    ) -> TransactionValidationOutcome<EthPooledTransaction> {
        Arc::make_mut(transaction.frame_validation.as_mut().unwrap()).head_hash = head_hash;
        TransactionValidationOutcome::Valid {
            balance: U256::MAX,
            state_nonce: transaction.frame_validation.as_ref().unwrap().state_nonce,
            bytecode_hash: None,
            transaction: ValidTransaction::Valid(transaction),
            propagate: true,
            authorities: None,
        }
    }

    fn frame_revalidation_pool(
        config: PoolConfig,
    ) -> (
        PoolInner<
            FrameRevalidationValidator,
            CoinbaseTipOrdering<EthPooledTransaction>,
            InMemoryBlobStore,
        >,
        EthPooledTransaction,
    ) {
        let pool = PoolInner::new(
            FrameRevalidationValidator::default(),
            CoinbaseTipOrdering::default(),
            InMemoryBlobStore::default(),
            config,
        );
        let transaction =
            frame_revalidation_transaction(B256::ZERO, 100, Address::repeat_byte(0x11), 0);
        pool.add_transactions(
            TransactionOrigin::External,
            [frame_revalidation_outcome(transaction.clone(), B256::ZERO)],
        )
        .pop()
        .unwrap()
        .unwrap();
        pool.blob_store
            .insert(
                *transaction.hash(),
                BlobTransactionSidecarVariant::Eip4844(BlobTransactionSidecar::default()).into(),
            )
            .unwrap();
        pool.set_block_info(BlockInfo {
            last_seen_block_hash: B256::repeat_byte(1),
            ..pool.block_info()
        });
        assert!(pool.get(transaction.hash()).is_none());
        assert_eq!(pool.frame_revalidation.lock().snapshot().len(), 1);
        (pool, transaction)
    }

    #[tokio::test]
    async fn frame_revalidation_retries_provider_head_ahead_of_pool() {
        let (pool, transaction) = frame_revalidation_pool(PoolConfig::default());
        let mut events = pool.add_all_transactions_event_listener();
        let provider_head = B256::repeat_byte(2);
        pool.validator
            .outcomes
            .lock()
            .push_back(frame_revalidation_outcome(transaction.clone(), provider_head));

        pool.revalidate_frame_transactions().await;

        assert!(pool.get(transaction.hash()).is_none());
        assert_eq!(pool.frame_revalidation.lock().snapshot().len(), 1);
        assert!(pool.blob_store.get(*transaction.hash()).unwrap().is_some());
        assert!(events.next().now_or_never().is_none());
        pool.set_block_info(BlockInfo { last_seen_block_hash: provider_head, ..pool.block_info() });
        pool.validator
            .outcomes
            .lock()
            .push_back(frame_revalidation_outcome(transaction.clone(), provider_head));
        pool.revalidate_frame_transactions().await;
        assert!(pool.get(transaction.hash()).is_some());
        assert!(pool.frame_revalidation.lock().snapshot().is_empty());
        assert!(pool.blob_store.get(*transaction.hash()).unwrap().is_some());
        assert!(
            matches!(events.next().now_or_never(), Some(Some(FullTransactionEvent::Pending(hash))) if hash == *transaction.hash())
        );
        assert!(events.next().now_or_never().is_none());
    }

    #[tokio::test]
    async fn frame_revalidation_retries_unavailable_and_provider_errors() {
        let (pool, transaction) = frame_revalidation_pool(PoolConfig::default());
        let mut events = pool.add_all_transactions_event_listener();
        for error in [
            FrameValidationError::Unavailable("canonical head unavailable"),
            FrameValidationError::other(std::io::Error::other("snapshot read failed")),
        ] {
            pool.validator.outcomes.lock().push_back(TransactionValidationOutcome::Error(
                *transaction.hash(),
                Box::new(error),
            ));
            pool.revalidate_frame_transactions().await;
            assert!(pool.get(transaction.hash()).is_none());
            assert_eq!(pool.frame_revalidation.lock().snapshot().len(), 1);
            assert!(pool.blob_store.get(*transaction.hash()).unwrap().is_some());
            assert!(events.next().now_or_never().is_none());
        }
        pool.validator.outcomes.lock().push_back(frame_revalidation_outcome(
            transaction.clone(),
            pool.block_info().last_seen_block_hash,
        ));
        pool.revalidate_frame_transactions().await;
        assert!(pool.get(transaction.hash()).is_some());
        assert!(pool.frame_revalidation.lock().snapshot().is_empty());
        assert!(pool.blob_store.get(*transaction.hash()).unwrap().is_some());
    }

    #[tokio::test]
    async fn frame_revalidation_defers_invalidation_when_pool_head_changes() {
        let (pool, transaction) = frame_revalidation_pool(PoolConfig::default());
        let (release, gate) = oneshot::channel();
        *pool.validator.gate.lock() = Some(gate);
        pool.validator.outcomes.lock().push_back(TransactionValidationOutcome::Invalid(
            transaction.clone(),
            InvalidPoolTransactionError::Underpriced,
        ));
        let revalidation = pool.revalidate_frame_transactions();
        tokio::pin!(revalidation);
        assert!(revalidation.as_mut().now_or_never().is_none());
        pool.set_block_info(BlockInfo {
            last_seen_block_hash: B256::repeat_byte(2),
            ..pool.block_info()
        });
        release.send(()).unwrap();
        revalidation.await;
        assert_eq!(pool.frame_revalidation.lock().snapshot().len(), 1);
        assert!(pool.blob_store.get(*transaction.hash()).unwrap().is_some());
        pool.validator.outcomes.lock().push_back(frame_revalidation_outcome(
            transaction.clone(),
            pool.block_info().last_seen_block_hash,
        ));
        pool.revalidate_frame_transactions().await;
        assert!(pool.get(transaction.hash()).is_some());
    }

    #[tokio::test]
    async fn frame_revalidation_terminal_invalidation_deletes_sidecar() {
        let (pool, transaction) = frame_revalidation_pool(PoolConfig::default());
        let mut events = pool.add_all_transactions_event_listener();
        pool.validator.outcomes.lock().push_back(TransactionValidationOutcome::Invalid(
            transaction.clone(),
            InvalidPoolTransactionError::Underpriced,
        ));
        pool.revalidate_frame_transactions().await;
        assert!(pool.get(transaction.hash()).is_none());
        assert!(pool.frame_revalidation.lock().snapshot().is_empty());
        assert!(pool.blob_store.get(*transaction.hash()).unwrap().is_none());
        assert!(
            matches!(events.next().now_or_never(), Some(Some(FullTransactionEvent::Invalid(hash))) if hash == *transaction.hash())
        );
        assert!(
            matches!(events.next().now_or_never(), Some(Some(FullTransactionEvent::Discarded(hash))) if hash == *transaction.hash())
        );
        assert!(events.next().now_or_never().is_none());
    }

    #[tokio::test]
    async fn frame_revalidation_does_not_restore_removed_inflight_candidate() {
        let (pool, transaction) = frame_revalidation_pool(PoolConfig::default());
        let (release, gate) = oneshot::channel();
        *pool.validator.gate.lock() = Some(gate);
        pool.validator.outcomes.lock().push_back(frame_revalidation_outcome(
            transaction.clone(),
            pool.block_info().last_seen_block_hash,
        ));
        let revalidation = pool.revalidate_frame_transactions();
        tokio::pin!(revalidation);
        assert!(revalidation.as_mut().now_or_never().is_none());
        let removed = pool.remove_transactions(vec![*transaction.hash()]);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].hash(), transaction.hash());
        release.send(()).unwrap();
        revalidation.await;
        assert!(pool.get(transaction.hash()).is_none());
        assert!(pool.frame_revalidation.lock().snapshot().is_empty());
    }

    #[tokio::test]
    async fn frame_revalidation_does_not_restore_replaced_inflight_candidate() {
        let (pool, transaction) = frame_revalidation_pool(PoolConfig::default());
        let (release, gate) = oneshot::channel();
        *pool.validator.gate.lock() = Some(gate);
        pool.validator.outcomes.lock().push_back(frame_revalidation_outcome(
            transaction.clone(),
            pool.block_info().last_seen_block_hash,
        ));
        let revalidation = pool.revalidate_frame_transactions();
        tokio::pin!(revalidation);
        assert!(revalidation.as_mut().now_or_never().is_none());
        let replacement = frame_revalidation_transaction(
            pool.block_info().last_seen_block_hash,
            200,
            transaction.sender(),
            0,
        );
        let replacement_hash = *replacement.hash();
        pool.add_transactions(
            TransactionOrigin::External,
            [frame_revalidation_outcome(replacement, pool.block_info().last_seen_block_hash)],
        )
        .pop()
        .unwrap()
        .unwrap();
        release.send(()).unwrap();
        revalidation.await;
        assert!(pool.get(transaction.hash()).is_none());
        assert!(pool.get(&replacement_hash).is_some());
        assert!(pool.frame_revalidation.lock().snapshot().is_empty());
        assert!(pool.blob_store.get(*transaction.hash()).unwrap().is_none());
    }

    #[test]
    fn frame_revalidation_capacity_eviction_releases_sidecars() {
        let config = PoolConfig {
            pending_limit: SubPoolLimit::new(1, usize::MAX),
            basefee_limit: SubPoolLimit::new(0, 0),
            queued_limit: SubPoolLimit::new(0, 0),
            blob_limit: SubPoolLimit::new(0, 0),
            ..Default::default()
        };
        let (pool, old) = frame_revalidation_pool(config);
        {
            let mut queue = pool.frame_revalidation.lock();
            let mut candidate = queue.remove(old.hash()).unwrap();
            Arc::make_mut(&mut candidate).timestamp =
                Instant::now() - std::time::Duration::from_secs(1);
            assert!(queue.insert(candidate).is_none());
        }
        let new = frame_revalidation_transaction(
            pool.block_info().last_seen_block_hash,
            100,
            Address::repeat_byte(0x22),
            0,
        );
        pool.add_transactions(
            TransactionOrigin::External,
            [frame_revalidation_outcome(new.clone(), pool.block_info().last_seen_block_hash)],
        )
        .pop()
        .unwrap()
        .unwrap();
        let mut events = pool.add_all_transactions_event_listener();

        pool.set_block_info(BlockInfo {
            last_seen_block_hash: B256::repeat_byte(2),
            ..pool.block_info()
        });

        let remaining = pool.frame_revalidation.lock().snapshot();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].hash(), new.hash());
        assert!(pool.blob_store.get(*old.hash()).unwrap().is_none());
        assert!(
            matches!(events.next().now_or_never(), Some(Some(FullTransactionEvent::Discarded(hash))) if hash == *old.hash())
        );
        assert!(events.next().now_or_never().is_none());
    }

    #[test]
    fn frame_revalidation_superseded_sender_releases_sidecars_on_each_withdrawal() {
        for withdrawal in ["block info", "canonical state", "accounts"] {
            let (pool, old) = frame_revalidation_pool(PoolConfig::default());
            let new = frame_revalidation_transaction(
                pool.block_info().last_seen_block_hash,
                100,
                old.sender(),
                1,
            );
            pool.add_transactions(
                TransactionOrigin::External,
                [frame_revalidation_outcome(new.clone(), pool.block_info().last_seen_block_hash)],
            )
            .pop()
            .unwrap()
            .unwrap();
            pool.blob_store
                .insert(
                    *new.hash(),
                    BlobTransactionSidecarVariant::Eip4844(BlobTransactionSidecar::default())
                        .into(),
                )
                .unwrap();
            let stale = pool.frame_revalidation.lock().snapshot().pop().unwrap();
            let mut events = pool.add_all_transactions_event_listener();

            match withdrawal {
                "block info" => pool.set_block_info(BlockInfo {
                    last_seen_block_hash: B256::repeat_byte(2),
                    ..pool.block_info()
                }),
                "canonical state" => {
                    let mut block = reth_ethereum_primitives::Block::default();
                    block.header.number = 2;
                    block.header.parent_hash = pool.block_info().last_seen_block_hash;
                    block.header.gas_limit = pool.block_info().block_gas_limit;
                    let new_tip = SealedBlock::seal_slow(block);
                    pool.on_canonical_state_change(CanonicalStateUpdate {
                        new_tip: &new_tip,
                        pending_block_base_fee: pool.block_info().pending_basefee,
                        pending_block_blob_fee: pool.block_info().pending_blob_fee,
                        changed_accounts: vec![],
                        mined_transactions: vec![],
                        update_kind: PoolUpdateKind::Commit,
                    });
                }
                "accounts" => pool.update_accounts(vec![ChangedAccount {
                    address: old.sender(),
                    nonce: 1,
                    balance: U256::MAX,
                }]),
                _ => unreachable!(),
            }

            let mut queue = pool.frame_revalidation.lock();
            let remaining = queue.snapshot();
            assert_eq!(remaining.len(), 1, "{withdrawal}");
            assert_eq!(remaining[0].hash(), new.hash(), "{withdrawal}");
            assert!(!queue.take_current(&stale), "{withdrawal}");
            assert!(pool.blob_store.get(*old.hash()).unwrap().is_none(), "{withdrawal}");
            assert!(pool.blob_store.get(*new.hash()).unwrap().is_some(), "{withdrawal}");
            assert!(
                matches!(events.next().now_or_never(), Some(Some(FullTransactionEvent::Discarded(hash))) if hash == *old.hash()),
                "{withdrawal}"
            );
            assert!(events.next().now_or_never().is_none(), "{withdrawal}");
        }
    }

    #[test]
    fn frame_revalidation_same_hash_refresh_keeps_sidecar_ownership() {
        let (pool, transaction) = frame_revalidation_pool(PoolConfig::default());
        let mut events = pool.add_all_transactions_event_listener();
        let mut queue = pool.frame_revalidation.lock();
        let stale = queue.snapshot().pop().unwrap();
        let refreshed = Arc::new((*stale).clone());

        assert!(queue.insert(Arc::clone(&refreshed)).is_none());

        assert!(!queue.take_current(&stale));
        let remaining = queue.snapshot();
        assert_eq!(remaining.len(), 1);
        assert!(Arc::ptr_eq(&remaining[0], &refreshed));
        assert!(pool.blob_store.get(*transaction.hash()).unwrap().is_some());
        assert!(events.next().now_or_never().is_none());
    }
}
