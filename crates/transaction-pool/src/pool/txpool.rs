//! The internal transaction pool implementation.

use crate::{
    config::{LocalTransactionConfig, TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER},
    error::{Eip4844PoolTransactionError, InvalidPoolTransactionError, PoolError, PoolErrorKind},
    identifier::{SenderId, TransactionId},
    metrics::TxPoolMetrics,
    pool::{
        best::BestTransactions,
        blob::BlobTransactions,
        parked::{BasefeeOrd, ParkedPool, QueuedOrd},
        pending::PendingPool,
        state::{SubPool, TxState},
        update::{Destination, PoolUpdate},
        AddedPendingTransaction, AddedTransaction, OnNewCanonicalStateOutcome,
    },
    traits::{BestTransactionsAttributes, BlockInfo, PoolSize},
    PoolConfig, PoolResult, PoolTransaction, PriceBumpConfig, TransactionOrdering,
    ValidPoolTransaction, U256,
};
use fnv::FnvHashMap;
use itertools::Itertools;
use reth_primitives::{
    constants::{
        eip4844::BLOB_TX_MIN_BLOB_GASPRICE, ETHEREUM_BLOCK_GAS_LIMIT, MIN_PROTOCOL_BASE_FEE,
    },
    Address, TxHash, B256,
};
use smallvec::SmallVec;
use std::{
    cmp::Ordering,
    collections::{btree_map::Entry, hash_map, BTreeMap, HashMap, HashSet},
    fmt,
    ops::Bound::{Excluded, Unbounded},
    sync::Arc,
};
use tracing::trace;

#[cfg_attr(doc, aquamarine::aquamarine)]
/// A pool that manages transactions.
///
/// This pool maintains the state of all transactions and stores them accordingly.
///
/// include_mmd!("docs/mermaid/txpool.mmd")
pub struct TxPool<T: TransactionOrdering> {
    /// Contains the currently known information about the senders.
    sender_info: FnvHashMap<SenderId, SenderInfo>,
    /// pending subpool
    ///
    /// Holds transactions that are ready to be executed on the current state.
    pending_pool: PendingPool<T>,
    /// Pool settings to enforce limits etc.
    config: PoolConfig,
    /// queued subpool
    ///
    /// Holds all parked transactions that depend on external changes from the sender:
    ///
    ///    - blocked by missing ancestor transaction (has nonce gaps)
    ///    - sender lacks funds to pay for this transaction.
    queued_pool: ParkedPool<QueuedOrd<T::Transaction>>,
    /// base fee subpool
    ///
    /// Holds all parked transactions that currently violate the dynamic fee requirement but could
    /// be moved to pending if the base fee changes in their favor (decreases) in future blocks.
    basefee_pool: ParkedPool<BasefeeOrd<T::Transaction>>,
    /// Blob transactions in the pool that are __not pending__.
    ///
    /// This means they either do not satisfy the dynamic fee requirement or the blob fee
    /// requirement. These transactions can be moved to pending if the base fee or blob fee changes
    /// in their favor (decreases) in future blocks. The transaction may need both the base fee and
    /// blob fee to decrease to become executable.
    blob_pool: BlobTransactions<T::Transaction>,
    /// All transactions in the pool.
    all_transactions: AllTransactions<T::Transaction>,
    /// Transaction pool metrics
    metrics: TxPoolMetrics,
}

// === impl TxPool ===

impl<T: TransactionOrdering> TxPool<T> {
    /// Create a new graph pool instance.
    pub fn new(ordering: T, config: PoolConfig) -> Self {
        Self {
            sender_info: Default::default(),
            pending_pool: PendingPool::new(ordering),
            queued_pool: Default::default(),
            basefee_pool: Default::default(),
            blob_pool: Default::default(),
            all_transactions: AllTransactions::new(&config),
            config,
            metrics: Default::default(),
        }
    }

    /// Retrieves the highest nonce for a specific sender from the transaction pool.
    pub fn get_highest_nonce_by_sender(&self, sender: SenderId) -> Option<u64> {
        self.all().txs_iter(sender).last().map(|(_, tx)| tx.transaction.nonce())
    }

    /// Retrieves the highest transaction (wrapped in an `Arc`) for a specific sender from the
    /// transaction pool.
    pub fn get_highest_transaction_by_sender(
        &self,
        sender: SenderId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.all().txs_iter(sender).last().map(|(_, tx)| Arc::clone(&tx.transaction))
    }

    /// Returns access to the [`AllTransactions`] container.
    pub(crate) const fn all(&self) -> &AllTransactions<T::Transaction> {
        &self.all_transactions
    }

    /// Returns all senders in the pool
    pub(crate) fn unique_senders(&self) -> HashSet<Address> {
        self.all_transactions.txs.values().map(|tx| tx.transaction.sender()).collect()
    }

    /// Returns stats about the size of pool.
    pub fn size(&self) -> PoolSize {
        PoolSize {
            pending: self.pending_pool.len(),
            pending_size: self.pending_pool.size(),
            basefee: self.basefee_pool.len(),
            basefee_size: self.basefee_pool.size(),
            queued: self.queued_pool.len(),
            queued_size: self.queued_pool.size(),
            blob: self.blob_pool.len(),
            blob_size: self.blob_pool.size(),
            total: self.all_transactions.len(),
        }
    }

    /// Returns the currently tracked block values
    pub const fn block_info(&self) -> BlockInfo {
        BlockInfo {
            last_seen_block_hash: self.all_transactions.last_seen_block_hash,
            last_seen_block_number: self.all_transactions.last_seen_block_number,
            pending_basefee: self.all_transactions.pending_fees.base_fee,
            pending_blob_fee: Some(self.all_transactions.pending_fees.blob_fee),
        }
    }

    /// Updates the tracked blob fee
    fn update_blob_fee(&mut self, mut pending_blob_fee: u128, base_fee_update: Ordering) {
        std::mem::swap(&mut self.all_transactions.pending_fees.blob_fee, &mut pending_blob_fee);
        match (self.all_transactions.pending_fees.blob_fee.cmp(&pending_blob_fee), base_fee_update)
        {
            (Ordering::Equal, Ordering::Equal) => {
                // fee unchanged, nothing to update
            }
            (Ordering::Greater, Ordering::Equal) |
            (Ordering::Equal, Ordering::Greater) |
            (Ordering::Greater, Ordering::Greater) => {
                // increased blob fee: recheck pending pool and remove all that are no longer valid
                let removed =
                    self.pending_pool.update_blob_fee(self.all_transactions.pending_fees.blob_fee);
                for tx in removed {
                    let to = {
                        let tx =
                            self.all_transactions.txs.get_mut(tx.id()).expect("tx exists in set");

                        // we unset the blob fee cap block flag, if the base fee is too high now
                        tx.state.remove(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK);
                        tx.subpool = tx.state.into();
                        tx.subpool
                    };
                    self.add_transaction_to_subpool(to, tx);
                }
            }
            (Ordering::Less, Ordering::Equal) | (_, Ordering::Less) => {
                // decreased blob fee or base fee: recheck blob pool and promote all that are now
                // valid
                let removed =
                    self.blob_pool.enforce_pending_fees(&self.all_transactions.pending_fees);
                for tx in removed {
                    let to = {
                        let tx =
                            self.all_transactions.txs.get_mut(tx.id()).expect("tx exists in set");
                        tx.state.insert(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK);
                        tx.state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
                        tx.subpool = tx.state.into();
                        tx.subpool
                    };
                    self.add_transaction_to_subpool(to, tx);
                }
            }
            (Ordering::Less, Ordering::Greater) => {
                // increased blob fee: recheck pending pool and remove all that are no longer valid
                let removed =
                    self.pending_pool.update_blob_fee(self.all_transactions.pending_fees.blob_fee);
                for tx in removed {
                    let to = {
                        let tx =
                            self.all_transactions.txs.get_mut(tx.id()).expect("tx exists in set");

                        // we unset the blob fee cap block flag, if the base fee is too high now
                        tx.state.remove(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK);
                        tx.subpool = tx.state.into();
                        tx.subpool
                    };
                    self.add_transaction_to_subpool(to, tx);
                }

                // decreased blob fee or base fee: recheck blob pool and promote all that are now
                // valid
                let removed =
                    self.blob_pool.enforce_pending_fees(&self.all_transactions.pending_fees);
                for tx in removed {
                    let to = {
                        let tx =
                            self.all_transactions.txs.get_mut(tx.id()).expect("tx exists in set");
                        tx.state.insert(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK);
                        tx.state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
                        tx.subpool = tx.state.into();
                        tx.subpool
                    };
                    self.add_transaction_to_subpool(to, tx);
                }
            }
        }
    }

    /// Updates the tracked basefee
    ///
    /// Depending on the change in direction of the basefee, this will promote or demote
    /// transactions from the basefee pool.
    fn update_basefee(&mut self, mut pending_basefee: u64) -> Ordering {
        std::mem::swap(&mut self.all_transactions.pending_fees.base_fee, &mut pending_basefee);
        match self.all_transactions.pending_fees.base_fee.cmp(&pending_basefee) {
            Ordering::Equal => {
                // fee unchanged, nothing to update
                Ordering::Equal
            }
            Ordering::Greater => {
                // increased base fee: recheck pending pool and remove all that are no longer valid
                let removed =
                    self.pending_pool.update_base_fee(self.all_transactions.pending_fees.base_fee);
                for tx in removed {
                    let to = {
                        let tx =
                            self.all_transactions.txs.get_mut(tx.id()).expect("tx exists in set");
                        tx.state.remove(TxState::ENOUGH_FEE_CAP_BLOCK);
                        tx.subpool = tx.state.into();
                        tx.subpool
                    };
                    self.add_transaction_to_subpool(to, tx);
                }

                Ordering::Greater
            }
            Ordering::Less => {
                // decreased base fee: recheck basefee pool and promote all that are now valid
                let removed =
                    self.basefee_pool.enforce_basefee(self.all_transactions.pending_fees.base_fee);
                for tx in removed {
                    let to = {
                        let tx =
                            self.all_transactions.txs.get_mut(tx.id()).expect("tx exists in set");
                        tx.state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
                        tx.subpool = tx.state.into();
                        tx.subpool
                    };
                    self.add_transaction_to_subpool(to, tx);
                }

                Ordering::Less
            }
        }
    }

    /// Sets the current block info for the pool.
    ///
    /// This will also apply updates to the pool based on the new base fee
    pub fn set_block_info(&mut self, info: BlockInfo) {
        let BlockInfo {
            last_seen_block_hash,
            last_seen_block_number,
            pending_basefee,
            pending_blob_fee,
        } = info;
        self.all_transactions.last_seen_block_hash = last_seen_block_hash;
        self.all_transactions.last_seen_block_number = last_seen_block_number;
        let basefee_ordering = self.update_basefee(pending_basefee);

        if let Some(blob_fee) = pending_blob_fee {
            self.update_blob_fee(blob_fee, basefee_ordering)
        }
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block with
    /// the tracked fees.
    pub(crate) fn best_transactions(&self) -> BestTransactions<T> {
        self.pending_pool.best()
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block with
    /// the given base fee and optional blob fee.
    ///
    /// If the provided attributes differ from the currently tracked fees, this will also include
    /// transactions that are unlocked by the new fees, or exclude transactions that are no longer
    /// valid with the new fees.
    pub(crate) fn best_transactions_with_attributes(
        &self,
        best_transactions_attributes: BestTransactionsAttributes,
    ) -> Box<dyn crate::traits::BestTransactions<Item = Arc<ValidPoolTransaction<T::Transaction>>>>
    {
        // First we need to check if the given base fee is different than what's currently being
        // tracked
        match best_transactions_attributes.basefee.cmp(&self.all_transactions.pending_fees.base_fee)
        {
            Ordering::Equal => {
                // for EIP-4844 transactions we also need to check if the blob fee is now lower than
                // what's currently being tracked, if so we need to include transactions from the
                // blob pool that are valid with the lower blob fee
                if best_transactions_attributes
                    .blob_fee
                    .map_or(false, |fee| fee < self.all_transactions.pending_fees.blob_fee as u64)
                {
                    let unlocked_by_blob_fee =
                        self.blob_pool.satisfy_attributes(best_transactions_attributes);

                    Box::new(self.pending_pool.best_with_unlocked(
                        unlocked_by_blob_fee,
                        self.all_transactions.pending_fees.base_fee,
                    ))
                } else {
                    Box::new(self.pending_pool.best())
                }
            }
            Ordering::Greater => {
                // base fee increased, we only need to enforce this on the pending pool
                Box::new(self.pending_pool.best_with_basefee_and_blobfee(
                    best_transactions_attributes.basefee,
                    best_transactions_attributes.blob_fee.unwrap_or_default(),
                ))
            }
            Ordering::Less => {
                // base fee decreased, we need to move transactions from the basefee + blob pool to
                // the pending pool that might be unlocked by the lower base fee
                let mut unlocked = self
                    .basefee_pool
                    .satisfy_base_fee_transactions(best_transactions_attributes.basefee);

                // also include blob pool transactions that are now unlocked
                unlocked.extend(self.blob_pool.satisfy_attributes(best_transactions_attributes));

                Box::new(
                    self.pending_pool
                        .best_with_unlocked(unlocked, self.all_transactions.pending_fees.base_fee),
                )
            }
        }
    }

    /// Returns all transactions from the pending sub-pool
    pub(crate) fn pending_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.pending_pool.all().collect()
    }

    /// Returns all transactions from parked pools
    pub(crate) fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.basefee_pool.all().chain(self.queued_pool.all()).collect()
    }

    /// Returns queued and pending transactions for the specified sender
    pub fn queued_and_pending_txs_by_sender(
        &self,
        sender: SenderId,
    ) -> (SmallVec<[TransactionId; TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER]>, Vec<TransactionId>) {
        (self.queued_pool.get_txs_by_sender(sender), self.pending_pool.get_txs_by_sender(sender))
    }

    /// Returns `true` if the transaction with the given hash is already included in this pool.
    pub(crate) fn contains(&self, tx_hash: &TxHash) -> bool {
        self.all_transactions.contains(tx_hash)
    }

    /// Returns `true` if the transaction with the given id is already included in the given subpool
    #[cfg(test)]
    pub(crate) fn subpool_contains(&self, subpool: SubPool, id: &TransactionId) -> bool {
        match subpool {
            SubPool::Queued => self.queued_pool.contains(id),
            SubPool::Pending => self.pending_pool.contains(id),
            SubPool::BaseFee => self.basefee_pool.contains(id),
            SubPool::Blob => self.blob_pool.contains(id),
        }
    }

    /// Returns `true` if the pool is over its configured limits.
    #[inline]
    pub(crate) fn is_exceeded(&self) -> bool {
        self.config.is_exceeded(self.size())
    }

    /// Returns the transaction for the given hash.
    pub(crate) fn get(
        &self,
        tx_hash: &TxHash,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.all_transactions.by_hash.get(tx_hash).cloned()
    }

    /// Returns transactions for the multiple given hashes, if they exist.
    pub(crate) fn get_all(
        &self,
        txs: Vec<TxHash>,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + '_ {
        txs.into_iter().filter_map(|tx| self.get(&tx))
    }

    /// Returns all transactions sent from the given sender.
    pub(crate) fn get_transactions_by_sender(
        &self,
        sender: SenderId,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.all_transactions.txs_iter(sender).map(|(_, tx)| Arc::clone(&tx.transaction)).collect()
    }

    /// Updates the transactions for the changed senders.
    pub(crate) fn update_accounts(
        &mut self,
        changed_senders: HashMap<SenderId, SenderInfo>,
    ) -> UpdateOutcome<T::Transaction> {
        // track changed accounts
        self.sender_info.extend(changed_senders.clone());
        // Apply the state changes to the total set of transactions which triggers sub-pool updates.
        let updates = self.all_transactions.update(changed_senders);
        // Process the sub-pool updates
        let update = self.process_updates(updates);
        // update the metrics after the update
        self.update_size_metrics();
        update
    }

    /// Updates the entire pool after a new block was mined.
    ///
    /// This removes all mined transactions, updates according to the new base fee and rechecks
    /// sender allowance.
    pub(crate) fn on_canonical_state_change(
        &mut self,
        block_info: BlockInfo,
        mined_transactions: Vec<TxHash>,
        changed_senders: HashMap<SenderId, SenderInfo>,
    ) -> OnNewCanonicalStateOutcome<T::Transaction> {
        // update block info
        let block_hash = block_info.last_seen_block_hash;
        self.all_transactions.set_block_info(block_info);

        // Remove all transaction that were included in the block
        for tx_hash in mined_transactions.iter() {
            if self.prune_transaction_by_hash(tx_hash).is_some() {
                // Update removed transactions metric
                self.metrics.removed_transactions.increment(1);
            }
        }

        let UpdateOutcome { promoted, discarded } = self.update_accounts(changed_senders);

        self.metrics.performed_state_updates.increment(1);

        OnNewCanonicalStateOutcome { block_hash, mined: mined_transactions, promoted, discarded }
    }

    /// Update sub-pools size metrics.
    pub(crate) fn update_size_metrics(&mut self) {
        let stats = self.size();
        self.metrics.pending_pool_transactions.set(stats.pending as f64);
        self.metrics.pending_pool_size_bytes.set(stats.pending_size as f64);
        self.metrics.basefee_pool_transactions.set(stats.basefee as f64);
        self.metrics.basefee_pool_size_bytes.set(stats.basefee_size as f64);
        self.metrics.queued_pool_transactions.set(stats.queued as f64);
        self.metrics.queued_pool_size_bytes.set(stats.queued_size as f64);
        self.metrics.blob_pool_transactions.set(stats.blob as f64);
        self.metrics.blob_pool_size_bytes.set(stats.blob_size as f64);
        self.metrics.total_transactions.set(stats.total as f64);
    }

    /// Adds the transaction into the pool.
    ///
    /// This pool consists of four sub-pools: `Queued`, `Pending`, `BaseFee`, and `Blob`.
    ///
    /// The `Queued` pool contains transactions with gaps in its dependency tree: It requires
    /// additional transactions that are note yet present in the pool. And transactions that the
    /// sender can not afford with the current balance.
    ///
    /// The `Pending` pool contains all transactions that have no nonce gaps, and can be afforded by
    /// the sender. It only contains transactions that are ready to be included in the pending
    /// block. The pending pool contains all transactions that could be listed currently, but not
    /// necessarily independently. However, this pool never contains transactions with nonce gaps. A
    /// transaction is considered `ready` when it has the lowest nonce of all transactions from the
    /// same sender. Which is equals to the chain nonce of the sender in the pending pool.
    ///
    /// The `BaseFee` pool contains transactions that currently can't satisfy the dynamic fee
    /// requirement. With EIP-1559, transactions can become executable or not without any changes to
    /// the sender's balance or nonce and instead their `feeCap` determines whether the
    /// transaction is _currently_ (on the current state) ready or needs to be parked until the
    /// `feeCap` satisfies the block's `baseFee`.
    ///
    /// The `Blob` pool contains _blob_ transactions that currently can't satisfy the dynamic fee
    /// requirement, or blob fee requirement. Transactions become executable only if the
    /// transaction `feeCap` is greater than the block's `baseFee` and the `maxBlobFee` is greater
    /// than the block's `blobFee`.
    pub(crate) fn add_transaction(
        &mut self,
        tx: ValidPoolTransaction<T::Transaction>,
        on_chain_balance: U256,
        on_chain_nonce: u64,
    ) -> PoolResult<AddedTransaction<T::Transaction>> {
        if self.contains(tx.hash()) {
            return Err(PoolError::new(*tx.hash(), PoolErrorKind::AlreadyImported))
        }

        // Update sender info with balance and nonce
        self.sender_info
            .entry(tx.sender_id())
            .or_default()
            .update(on_chain_nonce, on_chain_balance);

        match self.all_transactions.insert_tx(tx, on_chain_balance, on_chain_nonce) {
            Ok(InsertOk { transaction, move_to, replaced_tx, updates, .. }) => {
                // replace the new tx and remove the replaced in the subpool(s)
                self.add_new_transaction(transaction.clone(), replaced_tx.clone(), move_to);
                // Update inserted transactions metric
                self.metrics.inserted_transactions.increment(1);
                let UpdateOutcome { promoted, discarded } = self.process_updates(updates);

                let replaced = replaced_tx.map(|(tx, _)| tx);

                // This transaction was moved to the pending pool.
                let res = if move_to.is_pending() {
                    AddedTransaction::Pending(AddedPendingTransaction {
                        transaction,
                        promoted,
                        discarded,
                        replaced,
                    })
                } else {
                    AddedTransaction::Parked { transaction, subpool: move_to, replaced }
                };

                Ok(res)
            }
            Err(err) => {
                // Update invalid transactions metric
                self.metrics.invalid_transactions.increment(1);
                match err {
                    InsertErr::Underpriced { existing, transaction: _ } => {
                        Err(PoolError::new(existing, PoolErrorKind::ReplacementUnderpriced))
                    }
                    InsertErr::FeeCapBelowMinimumProtocolFeeCap { transaction, fee_cap } => {
                        Err(PoolError::new(
                            *transaction.hash(),
                            PoolErrorKind::FeeCapBelowMinimumProtocolFeeCap(fee_cap),
                        ))
                    }
                    InsertErr::ExceededSenderTransactionsCapacity { transaction } => {
                        Err(PoolError::new(
                            *transaction.hash(),
                            PoolErrorKind::SpammerExceededCapacity(transaction.sender()),
                        ))
                    }
                    InsertErr::TxGasLimitMoreThanAvailableBlockGas {
                        transaction,
                        block_gas_limit,
                        tx_gas_limit,
                    } => Err(PoolError::new(
                        *transaction.hash(),
                        PoolErrorKind::InvalidTransaction(
                            InvalidPoolTransactionError::ExceedsGasLimit(
                                block_gas_limit,
                                tx_gas_limit,
                            ),
                        ),
                    )),
                    InsertErr::BlobTxHasNonceGap { transaction } => Err(PoolError::new(
                        *transaction.hash(),
                        PoolErrorKind::InvalidTransaction(
                            Eip4844PoolTransactionError::Eip4844NonceGap.into(),
                        ),
                    )),
                    InsertErr::Overdraft { transaction } => Err(PoolError::new(
                        *transaction.hash(),
                        PoolErrorKind::InvalidTransaction(InvalidPoolTransactionError::Overdraft),
                    )),
                    InsertErr::TxTypeConflict { transaction } => Err(PoolError::new(
                        *transaction.hash(),
                        PoolErrorKind::ExistingConflictingTransactionType(
                            transaction.sender(),
                            transaction.tx_type(),
                        ),
                    )),
                }
            }
        }
    }

    /// Maintenance task to apply a series of updates.
    ///
    /// This will move/discard the given transaction according to the `PoolUpdate`
    fn process_updates(&mut self, updates: Vec<PoolUpdate>) -> UpdateOutcome<T::Transaction> {
        let mut outcome = UpdateOutcome::default();
        for PoolUpdate { id, hash, current, destination } in updates {
            match destination {
                Destination::Discard => {
                    // remove the transaction from the pool and subpool
                    if let Some(tx) = self.prune_transaction_by_hash(&hash) {
                        outcome.discarded.push(tx);
                    }
                    self.metrics.removed_transactions.increment(1);
                }
                Destination::Pool(move_to) => {
                    debug_assert_ne!(&move_to, &current, "destination must be different");
                    let moved = self.move_transaction(current, move_to, &id);
                    if matches!(move_to, SubPool::Pending) {
                        if let Some(tx) = moved {
                            outcome.promoted.push(tx);
                        }
                    }
                }
            }
        }
        outcome
    }

    /// Moves a transaction from one sub pool to another.
    ///
    /// This will remove the given transaction from one sub-pool and insert it into the other
    /// sub-pool.
    fn move_transaction(
        &mut self,
        from: SubPool,
        to: SubPool,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let tx = self.remove_from_subpool(from, id)?;
        self.add_transaction_to_subpool(to, tx.clone());
        Some(tx)
    }

    /// Removes and returns all matching transactions from the pool.
    ///
    /// Note: this does not advance any descendants of the removed transactions and does not apply
    /// any additional updates.
    pub(crate) fn remove_transactions(
        &mut self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        hashes.into_iter().filter_map(|hash| self.remove_transaction_by_hash(&hash)).collect()
    }

    /// Remove the transaction from the __entire__ pool.
    ///
    /// This includes the total set of transaction and the subpool it currently resides in.
    fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let (tx, pool) = self.all_transactions.remove_transaction(id)?;
        self.remove_from_subpool(pool, tx.id())
    }

    /// Remove the transaction from the entire pool via its hash.
    ///
    /// This includes the total set of transactions and the subpool it currently resides in.
    fn remove_transaction_by_hash(
        &mut self,
        tx_hash: &B256,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let (tx, pool) = self.all_transactions.remove_transaction_by_hash(tx_hash)?;
        self.remove_from_subpool(pool, tx.id())
    }

    /// This removes the transaction from the pool and advances any descendant state inside the
    /// subpool.
    ///
    /// This is intended to be used when a transaction is included in a block,
    /// [Self::on_canonical_state_change]
    fn prune_transaction_by_hash(
        &mut self,
        tx_hash: &B256,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let (tx, pool) = self.all_transactions.remove_transaction_by_hash(tx_hash)?;
        self.prune_from_subpool(pool, tx.id())
    }

    /// Removes the transaction from the given pool.
    ///
    /// Caution: this only removes the tx from the sub-pool and not from the pool itself
    fn remove_from_subpool(
        &mut self,
        pool: SubPool,
        tx: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        match pool {
            SubPool::Queued => self.queued_pool.remove_transaction(tx),
            SubPool::Pending => self.pending_pool.remove_transaction(tx),
            SubPool::BaseFee => self.basefee_pool.remove_transaction(tx),
            SubPool::Blob => self.blob_pool.remove_transaction(tx),
        }
    }

    /// Removes the transaction from the given pool and advance sub-pool internal state, with the
    /// expectation that the given transaction is included in a block.
    fn prune_from_subpool(
        &mut self,
        pool: SubPool,
        tx: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        match pool {
            SubPool::Pending => self.pending_pool.remove_transaction(tx),
            SubPool::Queued => self.queued_pool.remove_transaction(tx),
            SubPool::BaseFee => self.basefee_pool.remove_transaction(tx),
            SubPool::Blob => self.blob_pool.remove_transaction(tx),
        }
    }

    /// Removes _only_ the descendants of the given transaction from the __entire__ pool.
    ///
    /// All removed transactions are added to the `removed` vec.
    fn remove_descendants(
        &mut self,
        tx: &TransactionId,
        removed: &mut Vec<Arc<ValidPoolTransaction<T::Transaction>>>,
    ) {
        let mut id = *tx;

        // this will essentially pop _all_ descendant transactions one by one
        loop {
            let descendant =
                self.all_transactions.descendant_txs_exclusive(&id).map(|(id, _)| *id).next();
            if let Some(descendant) = descendant {
                if let Some(tx) = self.remove_transaction(&descendant) {
                    removed.push(tx)
                }
                id = descendant;
            } else {
                return
            }
        }
    }

    /// Inserts the transaction into the given sub-pool.
    fn add_transaction_to_subpool(
        &mut self,
        pool: SubPool,
        tx: Arc<ValidPoolTransaction<T::Transaction>>,
    ) {
        match pool {
            SubPool::Queued => {
                self.queued_pool.add_transaction(tx);
            }
            SubPool::Pending => {
                self.pending_pool.add_transaction(tx, self.all_transactions.pending_fees.base_fee);
            }
            SubPool::BaseFee => {
                self.basefee_pool.add_transaction(tx);
            }
            SubPool::Blob => {
                self.blob_pool.add_transaction(tx);
            }
        }
    }

    /// Inserts the transaction into the given sub-pool.
    /// Optionally, removes the replacement transaction.
    fn add_new_transaction(
        &mut self,
        transaction: Arc<ValidPoolTransaction<T::Transaction>>,
        replaced: Option<(Arc<ValidPoolTransaction<T::Transaction>>, SubPool)>,
        pool: SubPool,
    ) {
        if let Some((replaced, replaced_pool)) = replaced {
            // Remove the replaced transaction
            self.remove_from_subpool(replaced_pool, replaced.id());
        }

        self.add_transaction_to_subpool(pool, transaction)
    }

    /// Ensures that the transactions in the sub-pools are within the given bounds.
    ///
    /// If the current size exceeds the given bounds, the worst transactions are evicted from the
    /// pool and returned.
    ///
    /// This returns all transactions that were removed from the entire pool.
    pub(crate) fn discard_worst(&mut self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = Vec::new();

        // Helper macro that discards the worst transactions for the pools
        macro_rules! discard_worst {
            ($this:ident, $removed:ident, [$($limit:ident => $pool:ident),* $(,)*]) => {
                $ (
                while $this.$pool.exceeds(&$this.config.$limit)
                    {
                        trace!(
                            target: "txpool",
                            "discarding transactions from {}, limit: {:?}, curr size: {}, curr len: {}",
                            stringify!($pool),
                            $this.config.$limit,
                            $this.$pool.size(),
                            $this.$pool.len(),
                        );

                        // 1. first remove the worst transaction from the subpool
                        let removed_from_subpool = $this.$pool.truncate_pool($this.config.$limit.clone());

                        trace!(
                            target: "txpool",
                            "removed {} transactions from {}, limit: {:?}, curr size: {}, curr len: {}",
                            removed_from_subpool.len(),
                            stringify!($pool),
                            $this.config.$limit,
                            $this.$pool.size(),
                            $this.$pool.len()
                        );

                        // 2. remove all transactions from the total set
                        for tx in removed_from_subpool {
                            $this.all_transactions.remove_transaction(tx.id());

                            let id = *tx.id();

                            // keep track of removed transaction
                            removed.push(tx);

                            // 3. remove all its descendants from the entire pool
                            $this.remove_descendants(&id, &mut $removed);
                        }
                    }

                )*
            };
        }

        discard_worst!(
            self, removed, [
                pending_limit => pending_pool,
                basefee_limit => basefee_pool,
                blob_limit    => blob_pool,
                queued_limit  => queued_pool,
            ]
        );

        removed
    }

    /// Number of transactions in the entire pool
    pub(crate) fn len(&self) -> usize {
        self.all_transactions.len()
    }

    /// Whether the pool is empty
    pub(crate) fn is_empty(&self) -> bool {
        self.all_transactions.is_empty()
    }

    /// Asserts all invariants of the  pool's:
    ///
    ///  - All maps are bijections (`by_id`, `by_hash`)
    ///  - Total size is equal to the sum of all sub-pools
    ///
    /// # Panics
    /// if any invariant is violated
    #[cfg(any(test, feature = "test-utils"))]
    pub fn assert_invariants(&self) {
        let size = self.size();
        let actual = size.basefee + size.pending + size.queued + size.blob;
        assert_eq!(size.total, actual, "total size must be equal to the sum of all sub-pools, basefee:{}, pending:{}, queued:{}, blob:{}", size.basefee, size.pending, size.queued, size.blob);
        self.all_transactions.assert_invariants();
        self.pending_pool.assert_invariants();
        self.basefee_pool.assert_invariants();
        self.queued_pool.assert_invariants();
        self.blob_pool.assert_invariants();
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl TxPool<crate::test_utils::MockOrdering> {
    /// Creates a mock instance for testing.
    pub fn mock() -> Self {
        Self::new(crate::test_utils::MockOrdering::default(), PoolConfig::default())
    }
}

#[cfg(test)]
impl<T: TransactionOrdering> Drop for TxPool<T> {
    fn drop(&mut self) {
        self.assert_invariants();
    }
}

// Additional test impls
#[cfg(any(test, feature = "test-utils"))]
#[allow(dead_code)]
impl<T: TransactionOrdering> TxPool<T> {
    pub(crate) const fn pending(&self) -> &PendingPool<T> {
        &self.pending_pool
    }

    pub(crate) const fn base_fee(&self) -> &ParkedPool<BasefeeOrd<T::Transaction>> {
        &self.basefee_pool
    }

    pub(crate) const fn queued(&self) -> &ParkedPool<QueuedOrd<T::Transaction>> {
        &self.queued_pool
    }
}

impl<T: TransactionOrdering> fmt::Debug for TxPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxPool").field("config", &self.config).finish_non_exhaustive()
    }
}

/// Container for _all_ transaction in the pool.
///
/// This is the sole entrypoint that's guarding all sub-pools, all sub-pool actions are always
/// derived from this set. Updates returned from this type must be applied to the sub-pools.
pub(crate) struct AllTransactions<T: PoolTransaction> {
    /// Minimum base fee required by the protocol.
    ///
    /// Transactions with a lower base fee will never be included by the chain
    minimal_protocol_basefee: u64,
    /// The max gas limit of the block
    block_gas_limit: u64,
    /// Max number of executable transaction slots guaranteed per account
    max_account_slots: usize,
    /// _All_ transactions identified by their hash.
    by_hash: HashMap<TxHash, Arc<ValidPoolTransaction<T>>>,
    /// _All_ transaction in the pool sorted by their sender and nonce pair.
    txs: BTreeMap<TransactionId, PoolInternalTransaction<T>>,
    /// Tracks the number of transactions by sender that are currently in the pool.
    tx_counter: FnvHashMap<SenderId, usize>,
    /// The current block number the pool keeps track of.
    last_seen_block_number: u64,
    /// The current block hash the pool keeps track of.
    last_seen_block_hash: B256,
    /// Expected blob and base fee for the pending block.
    pending_fees: PendingFees,
    /// Configured price bump settings for replacements
    price_bumps: PriceBumpConfig,
    /// How to handle [TransactionOrigin::Local](crate::TransactionOrigin) transactions.
    local_transactions_config: LocalTransactionConfig,
}

impl<T: PoolTransaction> AllTransactions<T> {
    /// Create a new instance
    fn new(config: &PoolConfig) -> Self {
        Self {
            max_account_slots: config.max_account_slots,
            price_bumps: config.price_bumps,
            local_transactions_config: config.local_transactions_config.clone(),
            ..Default::default()
        }
    }

    /// Returns an iterator over all _unique_ hashes in the pool
    #[allow(dead_code)]
    pub(crate) fn hashes_iter(&self) -> impl Iterator<Item = TxHash> + '_ {
        self.by_hash.keys().copied()
    }

    /// Returns an iterator over all _unique_ hashes in the pool
    pub(crate) fn transactions_iter(
        &self,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T>>> + '_ {
        self.by_hash.values().cloned()
    }

    /// Returns if the transaction for the given hash is already included in this pool
    pub(crate) fn contains(&self, tx_hash: &TxHash) -> bool {
        self.by_hash.contains_key(tx_hash)
    }

    /// Returns the internal transaction with additional metadata
    pub(crate) fn get(&self, id: &TransactionId) -> Option<&PoolInternalTransaction<T>> {
        self.txs.get(id)
    }

    /// Increments the transaction counter for the sender
    pub(crate) fn tx_inc(&mut self, sender: SenderId) {
        let count = self.tx_counter.entry(sender).or_default();
        *count += 1;
    }

    /// Decrements the transaction counter for the sender
    pub(crate) fn tx_decr(&mut self, sender: SenderId) {
        if let hash_map::Entry::Occupied(mut entry) = self.tx_counter.entry(sender) {
            let count = entry.get_mut();
            if *count == 1 {
                entry.remove();
                return
            }
            *count -= 1;
        }
    }

    /// Updates the block specific info
    fn set_block_info(&mut self, block_info: BlockInfo) {
        let BlockInfo {
            last_seen_block_hash,
            last_seen_block_number,
            pending_basefee,
            pending_blob_fee,
        } = block_info;
        self.last_seen_block_number = last_seen_block_number;
        self.last_seen_block_hash = last_seen_block_hash;
        self.pending_fees.base_fee = pending_basefee;
        if let Some(pending_blob_fee) = pending_blob_fee {
            self.pending_fees.blob_fee = pending_blob_fee;
        }
    }

    /// Rechecks all transactions in the pool against the changes.
    ///
    /// Possible changes are:
    ///
    /// For all transactions:
    ///   - decreased basefee: promotes from `basefee` to `pending` sub-pool.
    ///   - increased basefee: demotes from `pending` to `basefee` sub-pool.
    /// Individually:
    ///   - decreased sender allowance: demote from (`basefee`|`pending`) to `queued`.
    ///   - increased sender allowance: promote from `queued` to
    ///       - `pending` if basefee condition is met.
    ///       - `basefee` if basefee condition is _not_ met.
    ///
    /// Additionally, this will also update the `cumulative_gas_used` for transactions of a sender
    /// that got transaction included in the block.
    pub(crate) fn update(
        &mut self,
        changed_accounts: HashMap<SenderId, SenderInfo>,
    ) -> Vec<PoolUpdate> {
        // pre-allocate a few updates
        let mut updates = Vec::with_capacity(64);

        let mut iter = self.txs.iter_mut().peekable();

        // Loop over all individual senders and update all affected transactions.
        // One sender may have up to `max_account_slots` transactions here, which means, worst case
        // `max_accounts_slots` need to be updated, for example if the first transaction is blocked
        // due to too low base fee.
        // However, we don't have to necessarily check every transaction of a sender. If no updates
        // are possible (nonce gap) then we can skip to the next sender.

        // The `unique_sender` loop will process the first transaction of all senders, update its
        // state and internally update all consecutive transactions
        'transactions: while let Some((id, tx)) = iter.next() {
            macro_rules! next_sender {
                ($iter:ident) => {
                    'this: while let Some((peek, _)) = iter.peek() {
                        if peek.sender != id.sender {
                            break 'this
                        }
                        iter.next();
                    }
                };
            }
            // tracks the balance if the sender was changed in the block
            let mut changed_balance = None;

            // check if this is a changed account
            if let Some(info) = changed_accounts.get(&id.sender) {
                // discard all transactions with a nonce lower than the current state nonce
                if id.nonce < info.state_nonce {
                    updates.push(PoolUpdate {
                        id: *tx.transaction.id(),
                        hash: *tx.transaction.hash(),
                        current: tx.subpool,
                        destination: Destination::Discard,
                    });
                    continue 'transactions
                }

                let ancestor = TransactionId::ancestor(id.nonce, info.state_nonce, id.sender);
                // If there's no ancestor then this is the next transaction.
                if ancestor.is_none() {
                    tx.state.insert(TxState::NO_NONCE_GAPS);
                    tx.state.insert(TxState::NO_PARKED_ANCESTORS);
                    tx.cumulative_cost = U256::ZERO;
                    if tx.transaction.cost() > info.balance {
                        // sender lacks sufficient funds to pay for this transaction
                        tx.state.remove(TxState::ENOUGH_BALANCE);
                    } else {
                        tx.state.insert(TxState::ENOUGH_BALANCE);
                    }
                }

                changed_balance = Some(info.balance);
            }

            // If there's a nonce gap, we can shortcircuit, because there's nothing to update yet.
            if tx.state.has_nonce_gap() {
                next_sender!(iter);
                continue 'transactions
            }

            // Since this is the first transaction of the sender, it has no parked ancestors
            tx.state.insert(TxState::NO_PARKED_ANCESTORS);

            // Update the first transaction of this sender.
            Self::update_tx_base_fee(self.pending_fees.base_fee, tx);
            // Track if the transaction's sub-pool changed.
            Self::record_subpool_update(&mut updates, tx);

            // Track blocking transactions.
            let mut has_parked_ancestor = !tx.state.is_pending();

            let mut cumulative_cost = tx.next_cumulative_cost();

            // the next expected nonce after this transaction: nonce + 1
            let mut next_nonce_in_line = tx.transaction.nonce().saturating_add(1);

            // Update all consecutive transaction of this sender
            while let Some((peek, ref mut tx)) = iter.peek_mut() {
                if peek.sender != id.sender {
                    // Found the next sender we need to check
                    continue 'transactions
                }

                if tx.transaction.nonce() == next_nonce_in_line {
                    // no longer nonce gapped
                    tx.state.insert(TxState::NO_NONCE_GAPS);
                } else {
                    // can short circuit if there's still a nonce gap
                    next_sender!(iter);
                    continue 'transactions
                }

                // update for next iteration of this sender's loop
                next_nonce_in_line = next_nonce_in_line.saturating_add(1);

                // update cumulative cost
                tx.cumulative_cost = cumulative_cost;
                // Update for next transaction
                cumulative_cost = tx.next_cumulative_cost();

                // If the account changed in the block, check the balance.
                if let Some(changed_balance) = changed_balance {
                    if cumulative_cost > changed_balance {
                        // sender lacks sufficient funds to pay for this transaction
                        tx.state.remove(TxState::ENOUGH_BALANCE);
                    } else {
                        tx.state.insert(TxState::ENOUGH_BALANCE);
                    }
                }

                // Update ancestor condition.
                if has_parked_ancestor {
                    tx.state.remove(TxState::NO_PARKED_ANCESTORS);
                } else {
                    tx.state.insert(TxState::NO_PARKED_ANCESTORS);
                }
                has_parked_ancestor = !tx.state.is_pending();

                // Update and record sub-pool changes.
                Self::update_tx_base_fee(self.pending_fees.base_fee, tx);
                Self::record_subpool_update(&mut updates, tx);

                // Advance iterator
                iter.next();
            }
        }

        updates
    }

    /// This will update the transaction's `subpool` based on its state.
    ///
    /// If the sub-pool derived from the state differs from the current pool, it will record a
    /// `PoolUpdate` for this transaction to move it to the new sub-pool.
    fn record_subpool_update(updates: &mut Vec<PoolUpdate>, tx: &mut PoolInternalTransaction<T>) {
        let current_pool = tx.subpool;
        tx.subpool = tx.state.into();
        if current_pool != tx.subpool {
            updates.push(PoolUpdate {
                id: *tx.transaction.id(),
                hash: *tx.transaction.hash(),
                current: current_pool,
                destination: Destination::Pool(tx.subpool),
            })
        }
    }

    /// Rechecks the transaction's dynamic fee condition.
    fn update_tx_base_fee(pending_block_base_fee: u64, tx: &mut PoolInternalTransaction<T>) {
        // Recheck dynamic fee condition.
        match tx.transaction.max_fee_per_gas().cmp(&(pending_block_base_fee as u128)) {
            Ordering::Greater | Ordering::Equal => {
                tx.state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
            }
            Ordering::Less => {
                tx.state.remove(TxState::ENOUGH_FEE_CAP_BLOCK);
            }
        }
    }

    /// Returns an iterator over all transactions for the given sender, starting with the lowest
    /// nonce
    pub(crate) fn txs_iter(
        &self,
        sender: SenderId,
    ) -> impl Iterator<Item = (&TransactionId, &PoolInternalTransaction<T>)> + '_ {
        self.txs
            .range((sender.start_bound(), Unbounded))
            .take_while(move |(other, _)| sender == other.sender)
    }

    /// Returns a mutable iterator over all transactions for the given sender, starting with the
    /// lowest nonce
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn txs_iter_mut(
        &mut self,
        sender: SenderId,
    ) -> impl Iterator<Item = (&TransactionId, &mut PoolInternalTransaction<T>)> + '_ {
        self.txs
            .range_mut((sender.start_bound(), Unbounded))
            .take_while(move |(other, _)| sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id and have the same sender.
    ///
    /// NOTE: The range is _exclusive_
    pub(crate) fn descendant_txs_exclusive<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + '_ {
        self.txs.range((Excluded(id), Unbounded)).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id but have the same sender.
    ///
    /// NOTE: The range is _inclusive_: if the transaction that belongs to `id` it will be the
    /// first value.
    pub(crate) fn descendant_txs_inclusive<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + '_ {
        self.txs.range(id..).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all mutable transactions that _follow_ after the given id but have the same sender.
    ///
    /// NOTE: The range is _inclusive_: if the transaction that belongs to `id` it field be the
    /// first value.
    pub(crate) fn descendant_txs_mut<'a, 'b: 'a>(
        &'a mut self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a mut PoolInternalTransaction<T>)> + '_ {
        self.txs.range_mut(id..).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Removes a transaction from the set using its hash.
    pub(crate) fn remove_transaction_by_hash(
        &mut self,
        tx_hash: &B256,
    ) -> Option<(Arc<ValidPoolTransaction<T>>, SubPool)> {
        let tx = self.by_hash.remove(tx_hash)?;
        let internal = self.txs.remove(&tx.transaction_id)?;
        // decrement the counter for the sender.
        self.tx_decr(tx.sender_id());
        Some((tx, internal.subpool))
    }

    /// Removes a transaction from the set.
    ///
    /// This will _not_ trigger additional updates, because descendants without nonce gaps are
    /// already in the pending pool, and this transaction will be the first transaction of the
    /// sender in this pool.
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<(Arc<ValidPoolTransaction<T>>, SubPool)> {
        let internal = self.txs.remove(id)?;

        // decrement the counter for the sender.
        self.tx_decr(internal.transaction.sender_id());

        self.by_hash.remove(internal.transaction.hash()).map(|tx| (tx, internal.subpool))
    }

    /// Checks if the given transaction's type conflicts with an existing transaction.
    ///
    /// See also [ValidPoolTransaction::tx_type_conflicts_with].
    ///
    /// Caution: This assumes that mutually exclusive invariant is always true for the same sender.
    #[inline]
    fn contains_conflicting_transaction(&self, tx: &ValidPoolTransaction<T>) -> bool {
        let mut iter = self.txs_iter(tx.transaction_id.sender);
        if let Some((_, existing)) = iter.next() {
            return tx.tx_type_conflicts_with(&existing.transaction)
        }
        // no existing transaction for this sender
        false
    }

    /// Additional checks for a new transaction.
    ///
    /// This will enforce all additional rules in the context of this pool, such as:
    ///   - Spam protection: reject new non-local transaction from a sender that exhausted its slot
    ///     capacity.
    ///   - Gas limit: reject transactions if they exceed a block's maximum gas.
    ///   - Ensures transaction types are not conflicting for the sender: blob vs normal
    ///     transactions are mutually exclusive for the same sender.
    fn ensure_valid(
        &self,
        transaction: ValidPoolTransaction<T>,
    ) -> Result<ValidPoolTransaction<T>, InsertErr<T>> {
        if !self.local_transactions_config.is_local(transaction.origin, transaction.sender()) {
            let current_txs =
                self.tx_counter.get(&transaction.sender_id()).copied().unwrap_or_default();
            if current_txs >= self.max_account_slots {
                return Err(InsertErr::ExceededSenderTransactionsCapacity {
                    transaction: Arc::new(transaction),
                })
            }
        }
        if transaction.gas_limit() > self.block_gas_limit {
            return Err(InsertErr::TxGasLimitMoreThanAvailableBlockGas {
                block_gas_limit: self.block_gas_limit,
                tx_gas_limit: transaction.gas_limit(),
                transaction: Arc::new(transaction),
            })
        }

        if self.contains_conflicting_transaction(&transaction) {
            // blob vs non blob transactions are mutually exclusive for the same sender
            return Err(InsertErr::TxTypeConflict { transaction: Arc::new(transaction) })
        }

        Ok(transaction)
    }

    /// Enforces additional constraints for blob transactions before attempting to insert:
    ///    - new blob transactions must not have any nonce gaps
    ///    - blob transactions cannot go into overdraft
    ///    - replacement blob transaction with a higher fee must not shift an already propagated
    ///      descending blob transaction into overdraft
    fn ensure_valid_blob_transaction(
        &self,
        new_blob_tx: ValidPoolTransaction<T>,
        on_chain_balance: U256,
        ancestor: Option<TransactionId>,
    ) -> Result<ValidPoolTransaction<T>, InsertErr<T>> {
        if let Some(ancestor) = ancestor {
            let Some(ancestor_tx) = self.txs.get(&ancestor) else {
                // ancestor tx is missing, so we can't insert the new blob
                return Err(InsertErr::BlobTxHasNonceGap { transaction: Arc::new(new_blob_tx) })
            };
            if ancestor_tx.state.has_nonce_gap() {
                // the ancestor transaction already has a nonce gap, so we can't insert the new
                // blob
                return Err(InsertErr::BlobTxHasNonceGap { transaction: Arc::new(new_blob_tx) })
            }

            // the max cost executing this transaction requires
            let mut cumulative_cost = ancestor_tx.next_cumulative_cost() + new_blob_tx.cost();

            // check if the new blob would go into overdraft
            if cumulative_cost > on_chain_balance {
                // the transaction would go into overdraft
                return Err(InsertErr::Overdraft { transaction: Arc::new(new_blob_tx) })
            }

            // ensure that a replacement would not shift already propagated blob transactions into
            // overdraft
            let id = new_blob_tx.transaction_id;
            let mut descendants = self.descendant_txs_inclusive(&id).peekable();
            if let Some((maybe_replacement, _)) = descendants.peek() {
                if **maybe_replacement == new_blob_tx.transaction_id {
                    // replacement transaction
                    descendants.next();

                    // check if any of descendant blob transactions should be shifted into overdraft
                    for (_, tx) in descendants {
                        cumulative_cost += tx.transaction.cost();
                        if tx.transaction.is_eip4844() && cumulative_cost > on_chain_balance {
                            // the transaction would shift
                            return Err(InsertErr::Overdraft { transaction: Arc::new(new_blob_tx) })
                        }
                    }
                }
            }
        } else if new_blob_tx.cost() > on_chain_balance {
            // the transaction would go into overdraft
            return Err(InsertErr::Overdraft { transaction: Arc::new(new_blob_tx) })
        }

        Ok(new_blob_tx)
    }

    /// Returns true if the replacement candidate is underpriced and can't replace the existing
    /// transaction.
    #[inline]
    fn is_underpriced(
        existing_transaction: &ValidPoolTransaction<T>,
        maybe_replacement: &ValidPoolTransaction<T>,
        price_bumps: &PriceBumpConfig,
    ) -> bool {
        let price_bump = price_bumps.price_bump(existing_transaction.tx_type());

        if maybe_replacement.max_fee_per_gas() <=
            existing_transaction.max_fee_per_gas() * (100 + price_bump) / 100
        {
            return true
        }

        let existing_max_priority_fee_per_gas =
            existing_transaction.transaction.max_priority_fee_per_gas().unwrap_or(0);
        let replacement_max_priority_fee_per_gas =
            maybe_replacement.transaction.max_priority_fee_per_gas().unwrap_or(0);

        if replacement_max_priority_fee_per_gas <=
            existing_max_priority_fee_per_gas * (100 + price_bump) / 100 &&
            existing_max_priority_fee_per_gas != 0 &&
            replacement_max_priority_fee_per_gas != 0
        {
            return true
        }

        // check max blob fee per gas
        if let Some(existing_max_blob_fee_per_gas) =
            existing_transaction.transaction.max_fee_per_blob_gas()
        {
            // this enforces that blob txs can only be replaced by blob txs
            let replacement_max_blob_fee_per_gas =
                maybe_replacement.transaction.max_fee_per_blob_gas().unwrap_or(0);
            if replacement_max_blob_fee_per_gas <=
                existing_max_blob_fee_per_gas * (100 + price_bump) / 100
            {
                return true
            }
        }

        false
    }

    /// Inserts a new _valid_ transaction into the pool.
    ///
    /// If the transaction already exists, it will be replaced if not underpriced.
    /// Returns info to which sub-pool the transaction should be moved.
    /// Also returns a set of pool updates triggered by this insert, that need to be handled by the
    /// caller.
    ///
    /// These can include:
    ///      - closing nonce gaps of descendant transactions
    ///      - enough balance updates
    ///
    /// Note: For EIP-4844 blob transactions additional constraints are enforced:
    ///      - new blob transactions must not have any nonce gaps
    ///      - blob transactions cannot go into overdraft
    ///
    /// ## Transaction type Exclusivity
    ///
    /// The pool enforces exclusivity of eip-4844 blob vs non-blob transactions on a per sender
    /// basis:
    ///   - If the pool already includes a blob transaction from the `transaction`'s sender, then
    ///     the  `transaction` must also be a blob transaction
    ///  - If the pool already includes a non-blob transaction from the `transaction`'s sender, then
    ///    the  `transaction` must _not_ be a blob transaction.
    ///
    /// In other words, the presence of blob transactions exclude non-blob transactions and vice
    /// versa:
    ///
    /// ## Replacements
    ///
    /// The replacement candidate must satisfy given price bump constraints: replacement candidate
    /// must not be underpriced
    pub(crate) fn insert_tx(
        &mut self,
        transaction: ValidPoolTransaction<T>,
        on_chain_balance: U256,
        on_chain_nonce: u64,
    ) -> InsertResult<T> {
        assert!(on_chain_nonce <= transaction.nonce(), "Invalid transaction");

        let mut transaction = self.ensure_valid(transaction)?;

        let inserted_tx_id = *transaction.id();
        let mut state = TxState::default();
        let mut cumulative_cost = U256::ZERO;
        let mut updates = Vec::new();

        // Current tx does not exceed block gas limit after ensure_valid check
        state.insert(TxState::NOT_TOO_MUCH_GAS);

        // identifier of the ancestor transaction, will be None if the transaction is the next tx of
        // the sender
        let ancestor = TransactionId::ancestor(
            transaction.transaction.nonce(),
            on_chain_nonce,
            inserted_tx_id.sender,
        );

        // before attempting to insert a blob transaction, we need to ensure that additional
        // constraints are met that only apply to blob transactions
        if transaction.is_eip4844() {
            state.insert(TxState::BLOB_TRANSACTION);

            transaction =
                self.ensure_valid_blob_transaction(transaction, on_chain_balance, ancestor)?;
            let blob_fee_cap = transaction.transaction.max_fee_per_blob_gas().unwrap_or_default();
            if blob_fee_cap >= self.pending_fees.blob_fee {
                state.insert(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK);
            }
        } else {
            // Non-EIP4844 transaction always satisfy the blob fee cap condition
            state.insert(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK);
        }

        let transaction = Arc::new(transaction);

        // If there's no ancestor tx then this is the next transaction.
        if ancestor.is_none() {
            state.insert(TxState::NO_NONCE_GAPS);
            state.insert(TxState::NO_PARKED_ANCESTORS);
        }

        // Check dynamic fee
        let fee_cap = transaction.max_fee_per_gas();

        if fee_cap < self.minimal_protocol_basefee as u128 {
            return Err(InsertErr::FeeCapBelowMinimumProtocolFeeCap { transaction, fee_cap })
        }
        if fee_cap >= self.pending_fees.base_fee as u128 {
            state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
        }

        // placeholder for the replaced transaction, if any
        let mut replaced_tx = None;

        let pool_tx = PoolInternalTransaction {
            transaction: Arc::clone(&transaction),
            subpool: state.into(),
            state,
            cumulative_cost,
        };

        // try to insert the transaction
        match self.txs.entry(*transaction.id()) {
            Entry::Vacant(entry) => {
                // Insert the transaction in both maps
                self.by_hash.insert(*pool_tx.transaction.hash(), pool_tx.transaction.clone());
                entry.insert(pool_tx);
            }
            Entry::Occupied(mut entry) => {
                // Transaction with the same nonce already exists: replacement candidate
                let existing_transaction = entry.get().transaction.as_ref();
                let maybe_replacement = transaction.as_ref();

                // Ensure the new transaction is not underpriced
                if Self::is_underpriced(existing_transaction, maybe_replacement, &self.price_bumps)
                {
                    return Err(InsertErr::Underpriced {
                        transaction: pool_tx.transaction,
                        existing: *entry.get().transaction.hash(),
                    })
                }
                let new_hash = *pool_tx.transaction.hash();
                let new_transaction = pool_tx.transaction.clone();
                let replaced = entry.insert(pool_tx);
                self.by_hash.remove(replaced.transaction.hash());
                self.by_hash.insert(new_hash, new_transaction);
                // also remove the hash
                replaced_tx = Some((replaced.transaction, replaced.subpool));
            }
        }

        // The next transaction of this sender
        let on_chain_id = TransactionId::new(transaction.sender_id(), on_chain_nonce);
        {
            // get all transactions of the sender's account
            let mut descendants = self.descendant_txs_mut(&on_chain_id).peekable();

            // Tracks the next nonce we expect if the transactions are gapless
            let mut next_nonce = on_chain_id.nonce;

            // We need to find out if the next transaction of the sender is considered pending
            //
            let mut has_parked_ancestor = if ancestor.is_none() {
                // the new transaction is the next one
                false
            } else {
                // The transaction was added above so the _inclusive_ descendants iterator
                // returns at least 1 tx.
                let (id, tx) = descendants.peek().expect("includes >= 1");
                if id.nonce < inserted_tx_id.nonce {
                    !tx.state.is_pending()
                } else {
                    true
                }
            };

            // Traverse all transactions of the sender and update existing transactions
            for (id, tx) in descendants {
                let current_pool = tx.subpool;

                // If there's a nonce gap, we can shortcircuit
                if next_nonce != id.nonce {
                    break
                }

                // close the nonce gap
                tx.state.insert(TxState::NO_NONCE_GAPS);

                // set cumulative cost
                tx.cumulative_cost = cumulative_cost;

                // Update for next transaction
                cumulative_cost = tx.next_cumulative_cost();

                if cumulative_cost > on_chain_balance {
                    // sender lacks sufficient funds to pay for this transaction
                    tx.state.remove(TxState::ENOUGH_BALANCE);
                } else {
                    tx.state.insert(TxState::ENOUGH_BALANCE);
                }

                // Update ancestor condition.
                if has_parked_ancestor {
                    tx.state.remove(TxState::NO_PARKED_ANCESTORS);
                } else {
                    tx.state.insert(TxState::NO_PARKED_ANCESTORS);
                }
                has_parked_ancestor = !tx.state.is_pending();

                // update the pool based on the state
                tx.subpool = tx.state.into();

                if inserted_tx_id.eq(id) {
                    // if it is the new transaction, track its updated state
                    state = tx.state;
                } else {
                    // check if anything changed
                    if current_pool != tx.subpool {
                        updates.push(PoolUpdate {
                            id: *id,
                            hash: *tx.transaction.hash(),
                            current: current_pool,
                            destination: Destination::Pool(tx.subpool),
                        })
                    }
                }

                // increment for next iteration
                next_nonce = id.next_nonce();
            }
        }

        // If this wasn't a replacement transaction we need to update the counter.
        if replaced_tx.is_none() {
            self.tx_inc(inserted_tx_id.sender);
        }

        Ok(InsertOk { transaction, move_to: state.into(), state, replaced_tx, updates })
    }

    /// Number of transactions in the entire pool
    pub(crate) fn len(&self) -> usize {
        self.txs.len()
    }

    /// Whether the pool is empty
    pub(crate) fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    /// Asserts that the bijection between `by_hash` and `txs` is valid.
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn assert_invariants(&self) {
        assert_eq!(self.by_hash.len(), self.txs.len(), "by_hash.len() != txs.len()");
    }
}

#[cfg(test)]
impl<T: PoolTransaction> AllTransactions<T> {
    /// This function retrieves the number of transactions stored in the pool for a specific sender.
    ///
    /// If there are no transactions for the given sender, it returns zero by default.
    pub(crate) fn tx_count(&self, sender: SenderId) -> usize {
        self.tx_counter.get(&sender).copied().unwrap_or_default()
    }
}

impl<T: PoolTransaction> Default for AllTransactions<T> {
    fn default() -> Self {
        Self {
            max_account_slots: TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
            minimal_protocol_basefee: MIN_PROTOCOL_BASE_FEE,
            block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            by_hash: Default::default(),
            txs: Default::default(),
            tx_counter: Default::default(),
            last_seen_block_number: Default::default(),
            last_seen_block_hash: Default::default(),
            pending_fees: Default::default(),
            price_bumps: Default::default(),
            local_transactions_config: Default::default(),
        }
    }
}

/// Represents updated fees for the pending block.
#[derive(Debug, Clone)]
pub(crate) struct PendingFees {
    /// The pending base fee
    pub(crate) base_fee: u64,
    /// The pending blob fee
    pub(crate) blob_fee: u128,
}

impl Default for PendingFees {
    fn default() -> Self {
        PendingFees { base_fee: Default::default(), blob_fee: BLOB_TX_MIN_BLOB_GASPRICE }
    }
}

/// Result type for inserting a transaction
pub(crate) type InsertResult<T> = Result<InsertOk<T>, InsertErr<T>>;

/// Err variant of `InsertResult`
#[derive(Debug)]
pub(crate) enum InsertErr<T: PoolTransaction> {
    /// Attempted to replace existing transaction, but was underpriced
    Underpriced {
        #[allow(dead_code)]
        transaction: Arc<ValidPoolTransaction<T>>,
        existing: TxHash,
    },
    /// Attempted to insert a blob transaction with a nonce gap
    BlobTxHasNonceGap { transaction: Arc<ValidPoolTransaction<T>> },
    /// Attempted to insert a transaction that would overdraft the sender's balance at the time of
    /// insertion.
    Overdraft { transaction: Arc<ValidPoolTransaction<T>> },
    /// The transactions feeCap is lower than the chain's minimum fee requirement.
    ///
    /// See also [`MIN_PROTOCOL_BASE_FEE`]
    FeeCapBelowMinimumProtocolFeeCap { transaction: Arc<ValidPoolTransaction<T>>, fee_cap: u128 },
    /// Sender currently exceeds the configured limit for max account slots.
    ///
    /// The sender can be considered a spammer at this point.
    ExceededSenderTransactionsCapacity { transaction: Arc<ValidPoolTransaction<T>> },
    /// Transaction gas limit exceeds block's gas limit
    TxGasLimitMoreThanAvailableBlockGas {
        transaction: Arc<ValidPoolTransaction<T>>,
        block_gas_limit: u64,
        tx_gas_limit: u64,
    },
    /// Thrown if the mutual exclusivity constraint (blob vs normal transaction) is violated.
    TxTypeConflict { transaction: Arc<ValidPoolTransaction<T>> },
}

/// Transaction was successfully inserted into the pool
#[derive(Debug)]
pub(crate) struct InsertOk<T: PoolTransaction> {
    /// Ref to the inserted transaction.
    transaction: Arc<ValidPoolTransaction<T>>,
    /// Where to move the transaction to.
    move_to: SubPool,
    /// Current state of the inserted tx.
    #[allow(dead_code)]
    state: TxState,
    /// The transaction that was replaced by this.
    replaced_tx: Option<(Arc<ValidPoolTransaction<T>>, SubPool)>,
    /// Additional updates to transactions affected by this change.
    updates: Vec<PoolUpdate>,
}

/// The internal transaction typed used by `AllTransactions` which also additional info used for
/// determining the current state of the transaction.
#[derive(Debug)]
pub(crate) struct PoolInternalTransaction<T: PoolTransaction> {
    /// The actual transaction object.
    pub(crate) transaction: Arc<ValidPoolTransaction<T>>,
    /// The `SubPool` that currently contains this transaction.
    pub(crate) subpool: SubPool,
    /// Keeps track of the current state of the transaction and therefor in which subpool it should
    /// reside
    pub(crate) state: TxState,
    /// The total cost all transactions before this transaction.
    ///
    /// This is the combined `cost` of all transactions from the same sender that currently
    /// come before this transaction.
    pub(crate) cumulative_cost: U256,
}

// === impl PoolInternalTransaction ===

impl<T: PoolTransaction> PoolInternalTransaction<T> {
    fn next_cumulative_cost(&self) -> U256 {
        self.cumulative_cost + self.transaction.cost()
    }
}

/// Tracks the result after updating the pool
#[derive(Debug)]
pub(crate) struct UpdateOutcome<T: PoolTransaction> {
    /// transactions promoted to the pending pool
    pub(crate) promoted: Vec<Arc<ValidPoolTransaction<T>>>,
    /// transaction that failed and were discarded
    pub(crate) discarded: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> Default for UpdateOutcome<T> {
    fn default() -> Self {
        Self { promoted: vec![], discarded: vec![] }
    }
}

/// Represents the outcome of a prune
pub struct PruneResult<T: PoolTransaction> {
    /// A list of added transactions that a pruned marker satisfied
    pub promoted: Vec<AddedTransaction<T>>,
    /// all transactions that failed to be promoted and now are discarded
    pub failed: Vec<TxHash>,
    /// all transactions that were pruned from the ready pool
    pub pruned: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> fmt::Debug for PruneResult<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PruneResult")
            .field(
                "promoted",
                &format_args!("[{}]", self.promoted.iter().map(|tx| tx.hash()).format(", ")),
            )
            .field("failed", &self.failed)
            .field(
                "pruned",
                &format_args!(
                    "[{}]",
                    self.pruned.iter().map(|tx| tx.transaction.hash()).format(", ")
                ),
            )
            .finish()
    }
}

/// Stores relevant context about a sender.
#[derive(Debug, Clone, Default)]
pub(crate) struct SenderInfo {
    /// current nonce of the sender.
    pub(crate) state_nonce: u64,
    /// Balance of the sender at the current point.
    pub(crate) balance: U256,
}

// === impl SenderInfo ===

impl SenderInfo {
    /// Updates the info with the new values.
    fn update(&mut self, state_nonce: u64, balance: U256) {
        *self = Self { state_nonce, balance };
    }
}

#[cfg(test)]
mod tests {
    use reth_primitives::{address, TxType};

    use super::*;
    use crate::{
        test_utils::{MockOrdering, MockTransaction, MockTransactionFactory, MockTransactionSet},
        traits::TransactionOrigin,
        SubPoolLimit,
    };

    #[test]
    fn test_insert_blob() {
        let on_chain_balance = U256::MAX;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip4844().inc_price().inc_limit();
        let valid_tx = f.validated(tx);
        let InsertOk { updates, replaced_tx, move_to, state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert!(state.contains(TxState::ENOUGH_BALANCE));
        assert!(state.contains(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK));
        assert_eq!(move_to, SubPool::Pending);

        let inserted = pool.txs.get(&valid_tx.transaction_id).unwrap();
        assert_eq!(inserted.subpool, SubPool::Pending);
    }

    #[test]
    fn test_insert_blob_not_enough_blob_fee() {
        let on_chain_balance = U256::MAX;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions {
            pending_fees: PendingFees { blob_fee: 10_000_000, ..Default::default() },
            ..Default::default()
        };
        let tx = MockTransaction::eip4844().inc_price().inc_limit();
        pool.pending_fees.blob_fee = tx.max_fee_per_blob_gas().unwrap() + 1;
        let valid_tx = f.validated(tx);
        let InsertOk { state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert!(!state.contains(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK));

        let _ = pool.txs.get(&valid_tx.transaction_id).unwrap();
    }

    #[test]
    fn test_valid_tx_with_decreasing_blob_fee() {
        let on_chain_balance = U256::MAX;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions {
            pending_fees: PendingFees { blob_fee: 10_000_000, ..Default::default() },
            ..Default::default()
        };
        let tx = MockTransaction::eip4844().inc_price().inc_limit();

        pool.pending_fees.blob_fee = tx.max_fee_per_blob_gas().unwrap() + 1;
        let valid_tx = f.validated(tx.clone());
        let InsertOk { state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert!(!state.contains(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK));

        let _ = pool.txs.get(&valid_tx.transaction_id).unwrap();
        pool.remove_transaction(&valid_tx.transaction_id);

        pool.pending_fees.blob_fee = tx.max_fee_per_blob_gas().unwrap();
        let InsertOk { state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert!(state.contains(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK));
    }

    #[test]
    fn test_demote_valid_tx_with_increasing_blob_fee() {
        let on_chain_balance = U256::MAX;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());
        let tx = MockTransaction::eip4844().inc_price().inc_limit();

        // set block info so the tx is initially underpriced w.r.t. blob fee
        let mut block_info = pool.block_info();
        block_info.pending_blob_fee = Some(tx.max_fee_per_blob_gas().unwrap());
        pool.set_block_info(block_info);

        let validated = f.validated(tx.clone());
        let id = *validated.id();
        pool.add_transaction(validated, on_chain_balance, on_chain_nonce).unwrap();

        // assert pool lengths
        assert!(pool.blob_pool.is_empty());
        assert_eq!(pool.pending_pool.len(), 1);

        // check tx state and derived subpool
        let internal_tx = pool.all_transactions.txs.get(&id).unwrap();
        assert!(internal_tx.state.contains(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK));
        assert_eq!(internal_tx.subpool, SubPool::Pending);

        // set block info so the pools are updated
        block_info.pending_blob_fee = Some(tx.max_fee_per_blob_gas().unwrap() + 1);
        pool.set_block_info(block_info);

        // check that the tx is promoted
        let internal_tx = pool.all_transactions.txs.get(&id).unwrap();
        assert!(!internal_tx.state.contains(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK));
        assert_eq!(internal_tx.subpool, SubPool::Blob);

        // make sure the blob transaction was promoted into the pending pool
        assert_eq!(pool.blob_pool.len(), 1);
        assert!(pool.pending_pool.is_empty());
    }

    #[test]
    fn test_promote_valid_tx_with_decreasing_blob_fee() {
        let on_chain_balance = U256::MAX;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());
        let tx = MockTransaction::eip4844().inc_price().inc_limit();

        // set block info so the tx is initially underpriced w.r.t. blob fee
        let mut block_info = pool.block_info();
        block_info.pending_blob_fee = Some(tx.max_fee_per_blob_gas().unwrap() + 1);
        pool.set_block_info(block_info);

        let validated = f.validated(tx.clone());
        let id = *validated.id();
        pool.add_transaction(validated, on_chain_balance, on_chain_nonce).unwrap();

        // assert pool lengths
        assert!(pool.pending_pool.is_empty());
        assert_eq!(pool.blob_pool.len(), 1);

        // check tx state and derived subpool
        let internal_tx = pool.all_transactions.txs.get(&id).unwrap();
        assert!(!internal_tx.state.contains(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK));
        assert_eq!(internal_tx.subpool, SubPool::Blob);

        // set block info so the pools are updated
        block_info.pending_blob_fee = Some(tx.max_fee_per_blob_gas().unwrap());
        pool.set_block_info(block_info);

        // check that the tx is promoted
        let internal_tx = pool.all_transactions.txs.get(&id).unwrap();
        assert!(internal_tx.state.contains(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK));
        assert_eq!(internal_tx.subpool, SubPool::Pending);

        // make sure the blob transaction was promoted into the pending pool
        assert_eq!(pool.pending_pool.len(), 1);
        assert!(pool.blob_pool.is_empty());
    }

    /// A struct representing a txpool promotion test instance
    #[derive(Debug, PartialEq, Eq, Clone, Hash)]
    struct PromotionTest {
        /// The basefee at the start of the test
        basefee: u64,
        /// The blobfee at the start of the test
        blobfee: u128,
        /// The subpool at the start of the test
        subpool: SubPool,
        /// The basefee update
        basefee_update: u64,
        /// The blobfee update
        blobfee_update: u128,
        /// The subpool after the update
        new_subpool: SubPool,
    }

    impl PromotionTest {
        /// Returns the test case for the opposite update
        const fn opposite(&self) -> Self {
            Self {
                basefee: self.basefee_update,
                blobfee: self.blobfee_update,
                subpool: self.new_subpool,
                blobfee_update: self.blobfee,
                basefee_update: self.basefee,
                new_subpool: self.subpool,
            }
        }

        fn assert_subpool_lengths<T: TransactionOrdering>(
            &self,
            pool: &TxPool<T>,
            failure_message: String,
            check_subpool: SubPool,
        ) {
            match check_subpool {
                SubPool::Blob => {
                    assert_eq!(pool.blob_pool.len(), 1, "{failure_message}");
                    assert!(pool.pending_pool.is_empty(), "{failure_message}");
                    assert!(pool.basefee_pool.is_empty(), "{failure_message}");
                    assert!(pool.queued_pool.is_empty(), "{failure_message}");
                }
                SubPool::Pending => {
                    assert!(pool.blob_pool.is_empty(), "{failure_message}");
                    assert_eq!(pool.pending_pool.len(), 1, "{failure_message}");
                    assert!(pool.basefee_pool.is_empty(), "{failure_message}");
                    assert!(pool.queued_pool.is_empty(), "{failure_message}");
                }
                SubPool::BaseFee => {
                    assert!(pool.blob_pool.is_empty(), "{failure_message}");
                    assert!(pool.pending_pool.is_empty(), "{failure_message}");
                    assert_eq!(pool.basefee_pool.len(), 1, "{failure_message}");
                    assert!(pool.queued_pool.is_empty(), "{failure_message}");
                }
                SubPool::Queued => {
                    assert!(pool.blob_pool.is_empty(), "{failure_message}");
                    assert!(pool.pending_pool.is_empty(), "{failure_message}");
                    assert!(pool.basefee_pool.is_empty(), "{failure_message}");
                    assert_eq!(pool.queued_pool.len(), 1, "{failure_message}");
                }
            }
        }

        /// Runs an assertion on the provided pool, ensuring that the transaction is in the correct
        /// subpool based on the starting condition of the test, assuming the pool contains only a
        /// single transaction.
        fn assert_single_tx_starting_subpool<T: TransactionOrdering>(&self, pool: &TxPool<T>) {
            self.assert_subpool_lengths(
                pool,
                format!("pool length check failed at start of test: {self:?}"),
                self.subpool,
            );
        }

        /// Runs an assertion on the provided pool, ensuring that the transaction is in the correct
        /// subpool based on the ending condition of the test, assuming the pool contains only a
        /// single transaction.
        fn assert_single_tx_ending_subpool<T: TransactionOrdering>(&self, pool: &TxPool<T>) {
            self.assert_subpool_lengths(
                pool,
                format!("pool length check failed at end of test: {self:?}"),
                self.new_subpool,
            );
        }
    }

    #[test]
    fn test_promote_blob_tx_with_both_pending_fee_updates() {
        // this exhaustively tests all possible promotion scenarios for a single transaction moving
        // between the blob and pending pool
        let on_chain_balance = U256::MAX;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let tx = MockTransaction::eip4844().inc_price().inc_limit();

        let max_fee_per_blob_gas = tx.max_fee_per_blob_gas().unwrap();
        let max_fee_per_gas = tx.max_fee_per_gas() as u64;

        // These are all _promotion_ tests or idempotent tests.
        let mut expected_promotions = vec![
            PromotionTest {
                blobfee: max_fee_per_blob_gas + 1,
                basefee: max_fee_per_gas + 1,
                subpool: SubPool::Blob,
                blobfee_update: max_fee_per_blob_gas + 1,
                basefee_update: max_fee_per_gas + 1,
                new_subpool: SubPool::Blob,
            },
            PromotionTest {
                blobfee: max_fee_per_blob_gas + 1,
                basefee: max_fee_per_gas + 1,
                subpool: SubPool::Blob,
                blobfee_update: max_fee_per_blob_gas,
                basefee_update: max_fee_per_gas + 1,
                new_subpool: SubPool::Blob,
            },
            PromotionTest {
                blobfee: max_fee_per_blob_gas + 1,
                basefee: max_fee_per_gas + 1,
                subpool: SubPool::Blob,
                blobfee_update: max_fee_per_blob_gas + 1,
                basefee_update: max_fee_per_gas,
                new_subpool: SubPool::Blob,
            },
            PromotionTest {
                blobfee: max_fee_per_blob_gas + 1,
                basefee: max_fee_per_gas + 1,
                subpool: SubPool::Blob,
                blobfee_update: max_fee_per_blob_gas,
                basefee_update: max_fee_per_gas,
                new_subpool: SubPool::Pending,
            },
            PromotionTest {
                blobfee: max_fee_per_blob_gas,
                basefee: max_fee_per_gas + 1,
                subpool: SubPool::Blob,
                blobfee_update: max_fee_per_blob_gas,
                basefee_update: max_fee_per_gas,
                new_subpool: SubPool::Pending,
            },
            PromotionTest {
                blobfee: max_fee_per_blob_gas + 1,
                basefee: max_fee_per_gas,
                subpool: SubPool::Blob,
                blobfee_update: max_fee_per_blob_gas,
                basefee_update: max_fee_per_gas,
                new_subpool: SubPool::Pending,
            },
            PromotionTest {
                blobfee: max_fee_per_blob_gas,
                basefee: max_fee_per_gas,
                subpool: SubPool::Pending,
                blobfee_update: max_fee_per_blob_gas,
                basefee_update: max_fee_per_gas,
                new_subpool: SubPool::Pending,
            },
        ];

        // extend the test cases with reversed updates - this will add all _demotion_ tests
        let reversed = expected_promotions.iter().map(|test| test.opposite()).collect::<Vec<_>>();
        expected_promotions.extend(reversed);

        // dedup the test cases
        let expected_promotions = expected_promotions.into_iter().collect::<HashSet<_>>();

        for promotion_test in expected_promotions.iter() {
            let mut pool = TxPool::new(MockOrdering::default(), Default::default());

            // set block info so the tx is initially underpriced w.r.t. blob fee
            let mut block_info = pool.block_info();

            block_info.pending_blob_fee = Some(promotion_test.blobfee);
            block_info.pending_basefee = promotion_test.basefee;
            pool.set_block_info(block_info);

            let validated = f.validated(tx.clone());
            let id = *validated.id();
            pool.add_transaction(validated, on_chain_balance, on_chain_nonce).unwrap();

            // assert pool lengths
            promotion_test.assert_single_tx_starting_subpool(&pool);

            // check tx state and derived subpool, it should not move into the blob pool
            let internal_tx = pool.all_transactions.txs.get(&id).unwrap();
            assert_eq!(
                internal_tx.subpool, promotion_test.subpool,
                "Subpools do not match at start of test: {promotion_test:?}"
            );

            // set block info with new base fee
            block_info.pending_basefee = promotion_test.basefee_update;
            block_info.pending_blob_fee = Some(promotion_test.blobfee_update);
            pool.set_block_info(block_info);

            // check tx state and derived subpool, it should not move into the blob pool
            let internal_tx = pool.all_transactions.txs.get(&id).unwrap();
            assert_eq!(
                internal_tx.subpool, promotion_test.new_subpool,
                "Subpools do not match at end of test: {promotion_test:?}"
            );

            // assert new pool lengths
            promotion_test.assert_single_tx_ending_subpool(&pool);
        }
    }

    #[test]
    fn test_insert_pending() {
        let on_chain_balance = U256::MAX;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let valid_tx = f.validated(tx);
        let InsertOk { updates, replaced_tx, move_to, state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert!(state.contains(TxState::ENOUGH_BALANCE));
        assert_eq!(move_to, SubPool::Pending);

        let inserted = pool.txs.get(&valid_tx.transaction_id).unwrap();
        assert_eq!(inserted.subpool, SubPool::Pending);
    }

    #[test]
    fn test_simple_insert() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let valid_tx = f.validated(tx.clone());
        let InsertOk { updates, replaced_tx, move_to, state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert!(!state.contains(TxState::ENOUGH_BALANCE));
        assert_eq!(move_to, SubPool::Queued);

        assert_eq!(pool.len(), 1);
        assert!(pool.contains(valid_tx.hash()));
        let expected_state = TxState::ENOUGH_FEE_CAP_BLOCK | TxState::NO_NONCE_GAPS;
        let inserted = pool.get(valid_tx.id()).unwrap();
        assert!(inserted.state.intersects(expected_state));

        // insert the same tx again
        let res = pool.insert_tx(valid_tx, on_chain_balance, on_chain_nonce);
        res.unwrap_err();
        assert_eq!(pool.len(), 1);

        let valid_tx = f.validated(tx.next());
        let InsertOk { updates, replaced_tx, move_to, state, .. } =
            pool.insert_tx(valid_tx.clone(), on_chain_balance, on_chain_nonce).unwrap();

        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert!(!state.contains(TxState::ENOUGH_BALANCE));
        assert_eq!(move_to, SubPool::Queued);

        assert!(pool.contains(valid_tx.hash()));
        assert_eq!(pool.len(), 2);
        let inserted = pool.get(valid_tx.id()).unwrap();
        assert!(inserted.state.intersects(expected_state));
    }

    #[test]
    fn insert_already_imported() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let tx = f.validated(tx);
        pool.add_transaction(tx.clone(), on_chain_balance, on_chain_nonce).unwrap();
        match pool.add_transaction(tx, on_chain_balance, on_chain_nonce).unwrap_err().kind {
            PoolErrorKind::AlreadyImported => {}
            _ => unreachable!(),
        }
    }

    #[test]
    fn insert_replace() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        let _ = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let replacement = f.validated(tx.rng_hash().inc_price());
        let InsertOk { updates, replaced_tx, .. } =
            pool.insert_tx(replacement.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(updates.is_empty());
        let replaced = replaced_tx.unwrap();
        assert_eq!(replaced.0.hash(), first.hash());

        // ensure replaced tx is fully removed
        assert!(!pool.contains(first.hash()));
        assert!(pool.contains(replacement.hash()));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn insert_replace_txpool() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::mock();

        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        let first_added =
            pool.add_transaction(first.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let replacement = f.validated(tx.rng_hash().inc_price());
        let replacement_added =
            pool.add_transaction(replacement.clone(), on_chain_balance, on_chain_nonce).unwrap();

        // // ensure replaced tx removed
        assert!(!pool.contains(first_added.hash()));
        // but the replacement is still there
        assert!(pool.subpool_contains(replacement_added.subpool(), replacement_added.id()));

        assert!(pool.contains(replacement.hash()));
        let size = pool.size();
        assert_eq!(size.total, 1);
        size.assert_invariants();
    }

    #[test]
    fn insert_replace_underpriced() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        let _res = pool.insert_tx(first, on_chain_balance, on_chain_nonce);
        let mut replacement = f.validated(tx.rng_hash());
        replacement.transaction = replacement.transaction.decr_price();
        let err = pool.insert_tx(replacement, on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::Underpriced { .. }));
    }

    #[test]
    fn insert_replace_underpriced_not_enough_bump() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let mut tx = MockTransaction::eip1559().inc_price().inc_limit();
        tx.set_priority_fee(100);
        tx.set_max_fee(100);
        let first = f.validated(tx.clone());
        let _ = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let mut replacement = f.validated(tx.rng_hash().inc_price());
        // a price bump of 9% is not enough for a default min price bump of 10%
        replacement.transaction.set_priority_fee(109);
        replacement.transaction.set_max_fee(109);
        let err =
            pool.insert_tx(replacement.clone(), on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::Underpriced { .. }));

        // ensure first tx is not removed
        assert!(pool.contains(first.hash()));
        assert_eq!(pool.len(), 1);

        // price bump of 10% is also not enough because the bump should be strictly greater than 10%
        replacement.transaction.set_priority_fee(110);
        replacement.transaction.set_max_fee(110);
        let err =
            pool.insert_tx(replacement.clone(), on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::Underpriced { .. }));
        assert!(pool.contains(first.hash()));
        assert_eq!(pool.len(), 1);

        // should also fail if the bump in priority fee is not enough
        replacement.transaction.set_priority_fee(111);
        replacement.transaction.set_max_fee(110);
        let err = pool.insert_tx(replacement, on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::Underpriced { .. }));
        assert!(pool.contains(first.hash()));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn insert_conflicting_type_normal_to_blob() {
        let on_chain_balance = U256::from(10_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        pool.insert_tx(first, on_chain_balance, on_chain_nonce).unwrap();
        let tx =
            MockTransaction::eip4844().set_sender(tx.get_sender()).inc_price_by(100).inc_limit();
        let blob = f.validated(tx);
        let err = pool.insert_tx(blob, on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::TxTypeConflict { .. }), "{:?}", err);
    }

    #[test]
    fn insert_conflicting_type_blob_to_normal() {
        let on_chain_balance = U256::from(10_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip4844().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        pool.insert_tx(first, on_chain_balance, on_chain_nonce).unwrap();
        let tx =
            MockTransaction::eip1559().set_sender(tx.get_sender()).inc_price_by(100).inc_limit();
        let tx = f.validated(tx);
        let err = pool.insert_tx(tx, on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::TxTypeConflict { .. }), "{:?}", err);
    }

    // insert nonce then nonce - 1
    #[test]
    fn insert_previous() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_nonce().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        let _res = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce);

        let first_in_pool = pool.get(first.id()).unwrap();

        // has nonce gap
        assert!(!first_in_pool.state.contains(TxState::NO_NONCE_GAPS));

        let prev = f.validated(tx.prev());
        let InsertOk { updates, replaced_tx, state, move_to, .. } =
            pool.insert_tx(prev, on_chain_balance, on_chain_nonce).unwrap();

        // no updates since still in queued pool
        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(move_to, SubPool::Queued);

        let first_in_pool = pool.get(first.id()).unwrap();
        // has non nonce gap
        assert!(first_in_pool.state.contains(TxState::NO_NONCE_GAPS));
    }

    // insert nonce then nonce - 1
    #[test]
    fn insert_with_updates() {
        let on_chain_balance = U256::from(10_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_nonce().set_gas_price(100).inc_limit();
        let first = f.validated(tx.clone());
        let _res = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce).unwrap();

        let first_in_pool = pool.get(first.id()).unwrap();
        // has nonce gap
        assert!(!first_in_pool.state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(SubPool::Queued, first_in_pool.subpool);

        let prev = f.validated(tx.prev());
        let InsertOk { updates, replaced_tx, state, move_to, .. } =
            pool.insert_tx(prev, on_chain_balance, on_chain_nonce).unwrap();

        // updated previous tx
        assert_eq!(updates.len(), 1);
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(move_to, SubPool::Pending);

        let first_in_pool = pool.get(first.id()).unwrap();
        // has non nonce gap
        assert!(first_in_pool.state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(SubPool::Pending, first_in_pool.subpool);
    }

    #[test]
    fn insert_previous_blocking() {
        let on_chain_balance = U256::from(1_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        pool.pending_fees.base_fee = pool.minimal_protocol_basefee.checked_add(1).unwrap();
        let tx = MockTransaction::eip1559().inc_nonce().inc_limit();
        let first = f.validated(tx.clone());

        let _res = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce);

        let first_in_pool = pool.get(first.id()).unwrap();

        assert!(tx.get_gas_price() < pool.pending_fees.base_fee as u128);
        // has nonce gap
        assert!(!first_in_pool.state.contains(TxState::NO_NONCE_GAPS));

        let prev = f.validated(tx.prev());
        let InsertOk { updates, replaced_tx, state, move_to, .. } =
            pool.insert_tx(prev, on_chain_balance, on_chain_nonce).unwrap();

        assert!(!state.contains(TxState::ENOUGH_FEE_CAP_BLOCK));
        // no updates since still in queued pool
        assert!(updates.is_empty());
        assert!(replaced_tx.is_none());
        assert!(state.contains(TxState::NO_NONCE_GAPS));
        assert_eq!(move_to, SubPool::BaseFee);

        let first_in_pool = pool.get(first.id()).unwrap();
        // has non nonce gap
        assert!(first_in_pool.state.contains(TxState::NO_NONCE_GAPS));
    }

    #[test]
    fn rejects_spammer() {
        let on_chain_balance = U256::from(1_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();

        let mut tx = MockTransaction::eip1559();
        for _ in 0..pool.max_account_slots {
            tx = tx.next();
            pool.insert_tx(f.validated(tx.clone()), on_chain_balance, on_chain_nonce).unwrap();
        }

        assert_eq!(
            pool.max_account_slots,
            pool.tx_count(f.ids.sender_id(&tx.get_sender()).unwrap())
        );

        let err =
            pool.insert_tx(f.validated(tx.next()), on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::ExceededSenderTransactionsCapacity { .. }));
    }

    #[test]
    fn allow_local_spamming() {
        let on_chain_balance = U256::from(1_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();

        let mut tx = MockTransaction::eip1559();
        for _ in 0..pool.max_account_slots {
            tx = tx.next();
            pool.insert_tx(
                f.validated_with_origin(TransactionOrigin::Local, tx.clone()),
                on_chain_balance,
                on_chain_nonce,
            )
            .unwrap();
        }

        assert_eq!(
            pool.max_account_slots,
            pool.tx_count(f.ids.sender_id(&tx.get_sender()).unwrap())
        );

        pool.insert_tx(
            f.validated_with_origin(TransactionOrigin::Local, tx.next()),
            on_chain_balance,
            on_chain_nonce,
        )
        .unwrap();
    }

    #[test]
    fn reject_tx_over_gas_limit() {
        let on_chain_balance = U256::from(1_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();

        let tx = MockTransaction::eip1559().with_gas_limit(30_000_001);

        assert!(matches!(
            pool.insert_tx(f.validated(tx), on_chain_balance, on_chain_nonce),
            Err(InsertErr::TxGasLimitMoreThanAvailableBlockGas { .. })
        ));
    }

    #[test]
    fn test_tx_equal_gas_limit() {
        let on_chain_balance = U256::from(1_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();

        let tx = MockTransaction::eip1559().with_gas_limit(30_000_000);

        let InsertOk { state, .. } =
            pool.insert_tx(f.validated(tx), on_chain_balance, on_chain_nonce).unwrap();
        assert!(state.contains(TxState::NOT_TOO_MUCH_GAS));
    }

    #[test]
    fn update_basefee_subpools() {
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx = MockTransaction::eip1559().inc_price_by(10);
        let validated = f.validated(tx.clone());
        let id = *validated.id();
        pool.add_transaction(validated, U256::from(1_000), 0).unwrap();

        assert_eq!(pool.pending_pool.len(), 1);

        pool.update_basefee((tx.max_fee_per_gas() + 1) as u64);

        assert!(pool.pending_pool.is_empty());
        assert_eq!(pool.basefee_pool.len(), 1);

        assert_eq!(pool.all_transactions.txs.get(&id).unwrap().subpool, SubPool::BaseFee)
    }

    #[test]
    fn update_basefee_subpools_setting_block_info() {
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx = MockTransaction::eip1559().inc_price_by(10);
        let validated = f.validated(tx.clone());
        let id = *validated.id();
        pool.add_transaction(validated, U256::from(1_000), 0).unwrap();

        assert_eq!(pool.pending_pool.len(), 1);

        // use set_block_info for the basefee update
        let mut block_info = pool.block_info();
        block_info.pending_basefee = (tx.max_fee_per_gas() + 1) as u64;
        pool.set_block_info(block_info);

        assert!(pool.pending_pool.is_empty());
        assert_eq!(pool.basefee_pool.len(), 1);

        assert_eq!(pool.all_transactions.txs.get(&id).unwrap().subpool, SubPool::BaseFee)
    }

    #[test]
    fn get_highest_transaction_by_sender_and_nonce() {
        // Set up a mock transaction factory and a new transaction pool.
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        // Create a mock transaction and add it to the pool.
        let tx = MockTransaction::eip1559();
        pool.add_transaction(f.validated(tx.clone()), U256::from(1_000), 0).unwrap();

        // Create another mock transaction with an incremented price.
        let tx1 = tx.inc_price().next().clone();

        // Validate the second mock transaction and add it to the pool.
        let tx1_validated = f.validated(tx1.clone());
        pool.add_transaction(tx1_validated, U256::from(1_000), 0).unwrap();

        // Ensure that the calculated next nonce for the sender matches the expected value.
        assert_eq!(
            pool.get_highest_nonce_by_sender(f.ids.sender_id(&tx.sender()).unwrap()),
            Some(1)
        );

        // Retrieve the highest transaction by sender.
        let highest_tx = pool
            .get_highest_transaction_by_sender(f.ids.sender_id(&tx.sender()).unwrap())
            .expect("Failed to retrieve highest transaction");

        // Validate that the retrieved highest transaction matches the expected transaction.
        assert_eq!(highest_tx.as_ref().transaction, tx1);
    }

    #[test]
    fn discard_nonce_too_low() {
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx = MockTransaction::eip1559().inc_price_by(10);
        let validated = f.validated(tx.clone());
        let id = *validated.id();
        pool.add_transaction(validated, U256::from(1_000), 0).unwrap();

        let next = tx.next();
        let validated = f.validated(next.clone());
        pool.add_transaction(validated, U256::from(1_000), 0).unwrap();

        assert_eq!(pool.pending_pool.len(), 2);

        let mut changed_senders = HashMap::new();
        changed_senders.insert(
            id.sender,
            SenderInfo { state_nonce: next.get_nonce(), balance: U256::from(1_000) },
        );
        let outcome = pool.update_accounts(changed_senders);
        assert_eq!(outcome.discarded.len(), 1);
        assert_eq!(pool.pending_pool.len(), 1);
    }

    #[test]
    fn discard_with_large_blob_txs() {
        // init tracing
        reth_tracing::init_test_tracing();

        // this test adds large txs to the parked pool, then attempting to discard worst
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());
        let default_limits = pool.config.blob_limit;

        // create a chain of transactions by sender A
        // make sure they are all one over half the limit
        let a_sender = address!("000000000000000000000000000000000000000a");

        // set the base fee of the pool
        let mut block_info = pool.block_info();
        block_info.pending_blob_fee = Some(100);
        block_info.pending_basefee = 100;

        // update
        pool.set_block_info(block_info);

        // 2 txs, that should put the pool over the size limit but not max txs
        let a_txs = MockTransactionSet::dependent(a_sender, 0, 2, TxType::EIP4844)
            .into_iter()
            .map(|mut tx| {
                tx.set_size(default_limits.max_size / 2 + 1);
                tx.set_max_fee((block_info.pending_basefee - 1).into());
                tx
            })
            .collect::<Vec<_>>();

        // add all the transactions to the parked pool
        for tx in a_txs {
            pool.add_transaction(f.validated(tx), U256::from(1_000), 0).unwrap();
        }

        // truncate the pool, it should remove at least one transaction
        let removed = pool.discard_worst();
        assert_eq!(removed.len(), 1);
    }

    #[test]
    fn discard_with_parked_large_txs() {
        // init tracing
        reth_tracing::init_test_tracing();

        // this test adds large txs to the parked pool, then attempting to discard worst
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());
        let default_limits = pool.config.queued_limit;

        // create a chain of transactions by sender A
        // make sure they are all one over half the limit
        let a_sender = address!("000000000000000000000000000000000000000a");

        // set the base fee of the pool
        let pool_base_fee = 100;
        pool.update_basefee(pool_base_fee);

        // 2 txs, that should put the pool over the size limit but not max txs
        let a_txs = MockTransactionSet::dependent(a_sender, 0, 2, TxType::EIP1559)
            .into_iter()
            .map(|mut tx| {
                tx.set_size(default_limits.max_size / 2 + 1);
                tx.set_max_fee((pool_base_fee - 1).into());
                tx
            })
            .collect::<Vec<_>>();

        // add all the transactions to the parked pool
        for tx in a_txs {
            pool.add_transaction(f.validated(tx), U256::from(1_000), 0).unwrap();
        }

        // truncate the pool, it should remove at least one transaction
        let removed = pool.discard_worst();
        assert_eq!(removed.len(), 1);
    }

    #[test]
    fn discard_at_capacity() {
        let mut f = MockTransactionFactory::default();
        let queued_limit = SubPoolLimit::new(1000, usize::MAX);
        let mut pool =
            TxPool::new(MockOrdering::default(), PoolConfig { queued_limit, ..Default::default() });

        // insert a bunch of transactions into the queued pool
        for _ in 0..queued_limit.max_txs {
            let tx = MockTransaction::eip1559().inc_price_by(10).inc_nonce();
            let validated = f.validated(tx.clone());
            let _id = *validated.id();
            pool.add_transaction(validated, U256::from(1_000), 0).unwrap();
        }

        let size = pool.size();
        assert_eq!(size.queued, queued_limit.max_txs);

        for _ in 0..queued_limit.max_txs {
            let tx = MockTransaction::eip1559().inc_price_by(10).inc_nonce();
            let validated = f.validated(tx.clone());
            let _id = *validated.id();
            pool.add_transaction(validated, U256::from(1_000), 0).unwrap();

            pool.discard_worst();
            pool.assert_invariants();
            assert!(pool.size().queued <= queued_limit.max_txs);
        }
    }

    #[test]
    fn discard_blobs_at_capacity() {
        let mut f = MockTransactionFactory::default();
        let blob_limit = SubPoolLimit::new(1000, usize::MAX);
        let mut pool =
            TxPool::new(MockOrdering::default(), PoolConfig { blob_limit, ..Default::default() });
        pool.all_transactions.pending_fees.blob_fee = 10000;
        // insert a bunch of transactions into the queued pool
        for _ in 0..blob_limit.max_txs {
            let tx = MockTransaction::eip4844().inc_price_by(100).with_blob_fee(100);
            let validated = f.validated(tx.clone());
            let _id = *validated.id();
            pool.add_transaction(validated, U256::from(1_000), 0).unwrap();
        }

        let size = pool.size();
        assert_eq!(size.blob, blob_limit.max_txs);

        for _ in 0..blob_limit.max_txs {
            let tx = MockTransaction::eip4844().inc_price_by(100).with_blob_fee(100);
            let validated = f.validated(tx.clone());
            let _id = *validated.id();
            pool.add_transaction(validated, U256::from(1_000), 0).unwrap();

            pool.discard_worst();
            pool.assert_invariants();
            assert!(pool.size().blob <= blob_limit.max_txs);
        }
    }

    #[test]
    fn account_updates_nonce_gap() {
        let on_chain_balance = U256::from(10_000);
        let mut on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx_0 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_1 = tx_0.next();
        let tx_2 = tx_1.next();

        // Create 4 transactions
        let v0 = f.validated(tx_0.clone());
        let v1 = f.validated(tx_1);
        let v2 = f.validated(tx_2);

        // Add first 2 to the pool
        let _res = pool.add_transaction(v0.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v1.clone(), on_chain_balance, on_chain_nonce).unwrap();

        assert!(pool.queued_transactions().is_empty());
        assert_eq!(2, pool.pending_transactions().len());

        // Remove first (nonce 0) - simulating that it was taken to be a part of the block.
        pool.prune_transaction_by_hash(v0.hash());

        // Now add transaction with nonce 2
        let _res = pool.add_transaction(v2.clone(), on_chain_balance, on_chain_nonce).unwrap();

        // v2 is in the queue now. v1 is still in 'pending'.
        assert_eq!(1, pool.queued_transactions().len());
        assert_eq!(1, pool.pending_transactions().len());

        // Simulate new block arrival - and chain nonce increasing.
        let mut updated_accounts = HashMap::new();
        on_chain_nonce += 1;
        updated_accounts.insert(
            v0.sender_id(),
            SenderInfo { state_nonce: on_chain_nonce, balance: on_chain_balance },
        );
        pool.update_accounts(updated_accounts);

        // 'pending' now).
        assert!(pool.queued_transactions().is_empty());
        assert_eq!(2, pool.pending_transactions().len());
    }
    #[test]
    fn test_transaction_removal() {
        let on_chain_balance = U256::from(10_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx_0 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_1 = tx_0.next();

        // Create 2 transactions
        let v0 = f.validated(tx_0.clone());
        let v1 = f.validated(tx_1);

        // Add them to the pool
        let _res = pool.add_transaction(v0.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v1.clone(), on_chain_balance, on_chain_nonce).unwrap();

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(2, pool.pending_transactions().len());

        // Remove first (nonce 0) - simulating that it was taken to be a part of the block.
        pool.remove_transaction(v0.id());
        // assert the second transaction is really at the top of the queue
        let pool_txs = pool.best_transactions().map(|x| x.id().nonce).collect::<Vec<_>>();
        assert_eq!(vec![v1.nonce()], pool_txs);
    }
    #[test]
    fn wrong_best_order_of_transactions() {
        let on_chain_balance = U256::from(10_000);
        let mut on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx_0 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_1 = tx_0.next();
        let tx_2 = tx_1.next();
        let tx_3 = tx_2.next();

        // Create 4 transactions
        let v0 = f.validated(tx_0.clone());
        let v1 = f.validated(tx_1);
        let v2 = f.validated(tx_2);
        let v3 = f.validated(tx_3);

        // Add first 2 to the pool
        let _res = pool.add_transaction(v0.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v1.clone(), on_chain_balance, on_chain_nonce).unwrap();

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(2, pool.pending_transactions().len());

        // Remove first (nonce 0) - simulating that it was taken to be a part of the block.
        pool.remove_transaction(v0.id());

        // Now add transaction with nonce 2
        let _res = pool.add_transaction(v2.clone(), on_chain_balance, on_chain_nonce).unwrap();

        // v2 is in the queue now. v1 is still in 'pending'.
        assert_eq!(1, pool.queued_transactions().len());
        assert_eq!(1, pool.pending_transactions().len());

        // Simulate new block arrival - and chain nonce increasing.
        let mut updated_accounts = HashMap::new();
        on_chain_nonce += 1;
        updated_accounts.insert(
            v0.sender_id(),
            SenderInfo { state_nonce: on_chain_nonce, balance: on_chain_balance },
        );
        pool.update_accounts(updated_accounts);

        // Transactions are not changed (IMHO - this is a bug, as transaction v2 should be in the
        // 'pending' now).
        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(2, pool.pending_transactions().len());

        // Add transaction v3 - it 'unclogs' everything.
        let _res = pool.add_transaction(v3.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(3, pool.pending_transactions().len());

        // It should have returned transactions in order (v1, v2, v3 - as there is nothing blocking
        // them).
        assert_eq!(
            pool.best_transactions().map(|x| x.id().nonce).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }
}
