//! The internal transaction pool implementation.

use crate::{
    config::TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER,
    error::{Eip4844PoolTransactionError, InvalidPoolTransactionError, PoolError, PoolErrorKind},
    identifier::{SenderId, TransactionId},
    metrics::TxPoolMetrics,
    pool::{
        all::{AllTransactions, InsertErr, InsertOk, SenderInfo},
        best::BestTransactions,
        blob::BlobTransactions,
        parked::{BasefeeOrd, ParkedPool, QueuedOrd},
        pending::PendingPool,
        state::{SubPool, TxState},
        update::{Destination, PoolUpdate, UpdateOutcome},
        AddedPendingTransaction, AddedTransaction, OnNewCanonicalStateOutcome,
    },
    traits::{BestTransactionsAttributes, BlockInfo, PoolSize},
    PoolConfig, PoolResult, PoolTransaction, PoolUpdateKind, TransactionOrdering,
    ValidPoolTransaction, U256,
};
use alloy_consensus::constants::{
    EIP1559_TX_TYPE_ID, EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID, EIP7702_TX_TYPE_ID,
    LEGACY_TX_TYPE_ID,
};
use alloy_eips::Typed2718;
use alloy_primitives::{Address, TxHash, B256};
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use std::{cmp::Ordering, collections::HashSet, fmt, sync::Arc};
use tracing::trace;

#[cfg_attr(doc, aquamarine::aquamarine)]
// TODO: Inlined diagram due to a bug in aquamarine library, should become an include when it's
// fixed. See https://github.com/mersinvald/aquamarine/issues/50
// include_mmd!("docs/mermaid/txpool.mmd")
/// A pool that manages transactions.
///
/// This pool maintains the state of all transactions and stores them accordingly.
///
/// ```mermaid
/// graph TB
///   subgraph TxPool
///     direction TB
///     pool[(All Transactions)]
///     subgraph Subpools
///         direction TB
///         B3[(Queued)]
///         B1[(Pending)]
///         B2[(Basefee)]
///         B4[(Blob)]
///     end
///   end
///   discard([discard])
///   production([Block Production])
///   new([New Block])
///   A[Incoming Tx] --> B[Validation] -->|ins
///   pool --> |if ready + blobfee too low| B4
///   pool --> |if ready| B1
///   pool --> |if ready + basfee too low| B2
///   pool --> |nonce gap or lack of funds| B3
///   pool --> |update| pool
///   B1 --> |best| production
///   B2 --> |worst| discard
///   B3 --> |worst| discard
///   B4 --> |worst| discard
///   B1 --> |increased blob fee| B4
///   B4 --> |decreased blob fee| B1
///   B1 --> |increased base fee| B2
///   B2 --> |decreased base fee| B1
///   B3 --> |promote| B1
///   B3 --> |promote| B2
///   new --> |apply state changes| pool
/// ```
pub struct TxPool<T: TransactionOrdering> {
    /// Contains the currently known information about the senders.
    sender_info: FxHashMap<SenderId, SenderInfo>,
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
    /// The last update kind that was applied to the pool.
    latest_update_kind: Option<PoolUpdateKind>,
}

// === impl TxPool ===

impl<T: TransactionOrdering> TxPool<T> {
    /// Create a new graph pool instance.
    pub fn new(ordering: T, config: PoolConfig) -> Self {
        Self {
            sender_info: Default::default(),
            pending_pool: PendingPool::with_buffer(
                ordering,
                config.max_new_pending_txs_notifications,
            ),
            queued_pool: Default::default(),
            basefee_pool: Default::default(),
            blob_pool: Default::default(),
            all_transactions: AllTransactions::new(&config),
            config,
            metrics: Default::default(),
            latest_update_kind: None,
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

    /// Returns the transaction with the highest nonce that is executable given the on chain nonce.
    ///
    /// If the pool already tracks a higher nonce for the given sender, then this nonce is used
    /// instead.
    ///
    /// Note: The next pending pooled transaction must have the on chain nonce.
    pub(crate) fn get_highest_consecutive_transaction_by_sender(
        &self,
        mut on_chain: TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut last_consecutive_tx = None;

        // ensure this operates on the most recent
        if let Some(current) = self.sender_info.get(&on_chain.sender) {
            on_chain.nonce = on_chain.nonce.max(current.state_nonce);
        }

        let mut next_expected_nonce = on_chain.nonce;
        for (id, tx) in self.all().descendant_txs_inclusive(&on_chain) {
            if next_expected_nonce != id.nonce {
                break
            }
            next_expected_nonce = id.next_nonce();
            last_consecutive_tx = Some(tx);
        }

        last_consecutive_tx.map(|tx| Arc::clone(&tx.transaction))
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
            block_gas_limit: self.all_transactions.block_gas_limit,
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
            (Ordering::Equal, Ordering::Equal | Ordering::Greater) => {
                // fee unchanged, nothing to update
            }
            (Ordering::Greater, Ordering::Equal | Ordering::Greater) => {
                // increased blob fee: recheck pending pool and remove all that are no longer valid
                let removed =
                    self.pending_pool.update_blob_fee(self.all_transactions.pending_fees.blob_fee);
                for tx in removed {
                    let to = {
                        let tx =
                            self.all_transactions.txs.get_mut(tx.id()).expect("tx exists in set");

                        // the blob fee is too high now, unset the blob fee cap block flag
                        tx.state.remove(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK);
                        tx.subpool = tx.state.into();
                        tx.subpool
                    };
                    self.add_transaction_to_subpool(to, tx);
                }
            }
            (Ordering::Less, _) | (_, Ordering::Less) => {
                // decreased blob/base fee: recheck blob pool and promote all that are now valid
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
            block_gas_limit,
            last_seen_block_hash,
            last_seen_block_number,
            pending_basefee,
            pending_blob_fee,
        } = info;
        self.all_transactions.last_seen_block_hash = last_seen_block_hash;
        self.all_transactions.last_seen_block_number = last_seen_block_number;
        let basefee_ordering = self.update_basefee(pending_basefee);

        self.all_transactions.block_gas_limit = block_gas_limit;

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
                    .is_some_and(|fee| fee < self.all_transactions.pending_fees.blob_fee as u64)
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
    /// Returns an iterator over all transactions from the pending sub-pool
    pub(crate) fn pending_transactions_iter(
        &self,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + '_ {
        self.pending_pool.all()
    }

    /// Returns all pending transactions filtered by predicate
    pub(crate) fn pending_transactions_with_predicate(
        &self,
        mut predicate: impl FnMut(&ValidPoolTransaction<T::Transaction>) -> bool,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.pending_transactions_iter().filter(|tx| predicate(tx)).collect()
    }

    /// Returns all pending transactions for the specified sender
    pub(crate) fn pending_txs_by_sender(
        &self,
        sender: SenderId,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.pending_transactions_iter().filter(|tx| tx.sender_id() == sender).collect()
    }

    /// Returns all transactions from parked pools
    pub(crate) fn queued_transactions(&self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.basefee_pool.all().chain(self.queued_pool.all()).collect()
    }

    /// Returns an iterator over all transactions from parked pools
    pub(crate) fn queued_transactions_iter(
        &self,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + '_ {
        self.basefee_pool.all().chain(self.queued_pool.all())
    }

    /// Returns queued and pending transactions for the specified sender
    pub fn queued_and_pending_txs_by_sender(
        &self,
        sender: SenderId,
    ) -> (SmallVec<[TransactionId; TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER]>, Vec<TransactionId>) {
        (self.queued_pool.get_txs_by_sender(sender), self.pending_pool.get_txs_by_sender(sender))
    }

    /// Returns all queued transactions for the specified sender
    pub(crate) fn queued_txs_by_sender(
        &self,
        sender: SenderId,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.queued_transactions_iter().filter(|tx| tx.sender_id() == sender).collect()
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
        changed_senders: FxHashMap<SenderId, SenderInfo>,
    ) -> UpdateOutcome<T::Transaction> {
        // Apply the state changes to the total set of transactions which triggers sub-pool updates.
        let updates = self.all_transactions.update(&changed_senders);

        // track changed accounts
        self.sender_info.extend(changed_senders);

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
        changed_senders: FxHashMap<SenderId, SenderInfo>,
        update_kind: PoolUpdateKind,
    ) -> OnNewCanonicalStateOutcome<T::Transaction> {
        // update block info
        let block_hash = block_info.last_seen_block_hash;
        self.all_transactions.set_block_info(block_info);

        // Remove all transaction that were included in the block
        let mut removed_txs_count = 0;
        for tx_hash in &mined_transactions {
            if self.prune_transaction_by_hash(tx_hash).is_some() {
                removed_txs_count += 1;
            }
        }

        // Update removed transactions metric
        self.metrics.removed_transactions.increment(removed_txs_count);

        let UpdateOutcome { promoted, discarded } = self.update_accounts(changed_senders);

        self.update_transaction_type_metrics();
        self.metrics.performed_state_updates.increment(1);

        // Update the latest update kind
        self.latest_update_kind = Some(update_kind);

        OnNewCanonicalStateOutcome { block_hash, mined: mined_transactions, promoted, discarded }
    }

    /// Update sub-pools size metrics.
    pub(crate) fn update_size_metrics(&self) {
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

    /// Updates transaction type metrics for the entire pool.
    pub(crate) fn update_transaction_type_metrics(&self) {
        let mut legacy_count = 0;
        let mut eip2930_count = 0;
        let mut eip1559_count = 0;
        let mut eip4844_count = 0;
        let mut eip7702_count = 0;

        for tx in self.all_transactions.transactions_iter() {
            match tx.transaction.ty() {
                LEGACY_TX_TYPE_ID => legacy_count += 1,
                EIP2930_TX_TYPE_ID => eip2930_count += 1,
                EIP1559_TX_TYPE_ID => eip1559_count += 1,
                EIP4844_TX_TYPE_ID => eip4844_count += 1,
                EIP7702_TX_TYPE_ID => eip7702_count += 1,
                _ => {} // Ignore other types
            }
        }

        self.metrics.total_legacy_transactions.set(legacy_count as f64);
        self.metrics.total_eip2930_transactions.set(eip2930_count as f64);
        self.metrics.total_eip1559_transactions.set(eip1559_count as f64);
        self.metrics.total_eip4844_transactions.set(eip4844_count as f64);
        self.metrics.total_eip7702_transactions.set(eip7702_count as f64);
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

                // Update size metrics after adding and potentially moving transactions.
                self.update_size_metrics();

                Ok(res)
            }
            Err(err) => {
                // Update invalid transactions metric
                self.metrics.invalid_transactions.increment(1);
                match err {
                    InsertErr::Underpriced { existing: _, transaction } => Err(PoolError::new(
                        *transaction.hash(),
                        PoolErrorKind::ReplacementUnderpriced,
                    )),
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
                                tx_gas_limit,
                                block_gas_limit,
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
                        PoolErrorKind::InvalidTransaction(InvalidPoolTransactionError::Overdraft {
                            cost: *transaction.cost(),
                            balance: on_chain_balance,
                        }),
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
                            trace!(target: "txpool", hash=%tx.transaction.hash(), "Promoted transaction to pending");
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
        let txs =
            hashes.into_iter().filter_map(|hash| self.remove_transaction_by_hash(&hash)).collect();
        self.update_size_metrics();
        txs
    }

    /// Removes and returns all matching transactions and their descendants from the pool.
    pub(crate) fn remove_transactions_and_descendants(
        &mut self,
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = Vec::new();
        for hash in hashes {
            if let Some(tx) = self.remove_transaction_by_hash(&hash) {
                removed.push(tx.clone());
                self.remove_descendants(tx.id(), &mut removed);
            }
        }
        self.update_size_metrics();
        removed
    }

    /// Removes all transactions from the given sender.
    pub(crate) fn remove_transactions_by_sender(
        &mut self,
        sender_id: SenderId,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = Vec::new();
        let txs = self.get_transactions_by_sender(sender_id);
        for tx in txs {
            if let Some(tx) = self.remove_transaction(tx.id()) {
                removed.push(tx);
            }
        }
        self.update_size_metrics();
        removed
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
    /// [`Self::on_canonical_state_change`]
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
        let tx = match pool {
            SubPool::Queued => self.queued_pool.remove_transaction(tx),
            SubPool::Pending => self.pending_pool.remove_transaction(tx),
            SubPool::BaseFee => self.basefee_pool.remove_transaction(tx),
            SubPool::Blob => self.blob_pool.remove_transaction(tx),
        };

        if let Some(ref tx) = tx {
            // We trace here instead of in subpool structs directly, because the `ParkedPool` type
            // is generic and it would not be possible to distinguish whether a transaction is
            // being removed from the `BaseFee` pool, or the `Queued` pool.
            trace!(target: "txpool", hash=%tx.transaction.hash(), ?pool, "Removed transaction from a subpool");
        }

        tx
    }

    /// Removes the transaction from the given pool and advance sub-pool internal state, with the
    /// expectation that the given transaction is included in a block.
    fn prune_from_subpool(
        &mut self,
        pool: SubPool,
        tx: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let tx = match pool {
            SubPool::Pending => self.pending_pool.remove_transaction(tx),
            SubPool::Queued => self.queued_pool.remove_transaction(tx),
            SubPool::BaseFee => self.basefee_pool.remove_transaction(tx),
            SubPool::Blob => self.blob_pool.remove_transaction(tx),
        };

        if let Some(ref tx) = tx {
            // We trace here instead of in subpool structs directly, because the `ParkedPool` type
            // is generic and it would not be possible to distinguish whether a transaction is
            // being pruned from the `BaseFee` pool, or the `Queued` pool.
            trace!(target: "txpool", hash=%tx.transaction.hash(), ?pool, "Pruned transaction from a subpool");
        }

        tx
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
        // We trace here instead of in structs directly, because the `ParkedPool` type is
        // generic and it would not be possible to distinguish whether a transaction is being
        // added to the `BaseFee` pool, or the `Queued` pool.
        trace!(target: "txpool", hash=%tx.transaction.hash(), ?pool, "Adding transaction to a subpool");
        match pool {
            SubPool::Queued => self.queued_pool.add_transaction(tx),
            SubPool::Pending => {
                self.pending_pool.add_transaction(tx, self.all_transactions.pending_fees.base_fee);
            }
            SubPool::BaseFee => self.basefee_pool.add_transaction(tx),
            SubPool::Blob => self.blob_pool.add_transaction(tx),
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
            ($this:ident, $removed:ident, [$($limit:ident => ($pool:ident, $metric:ident)),* $(,)*]) => {
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
                        $this.metrics.$metric.increment(removed_from_subpool.len() as u64);

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
                pending_limit => (pending_pool, pending_transactions_evicted),
                basefee_limit => (basefee_pool, basefee_transactions_evicted),
                blob_limit    => (blob_pool, blob_transactions_evicted),
                queued_limit  => (queued_pool, queued_transactions_evicted),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{MockOrdering, MockTransaction, MockTransactionFactory, MockTransactionSet},
        SubPoolLimit,
    };
    use alloy_consensus::{Transaction, TxType};
    use alloy_primitives::address;
    use std::collections::HashMap;

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

        for promotion_test in &expected_promotions {
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
    fn insert_replace_txpool() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::mock();

        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        let first_added = pool.add_transaction(first, on_chain_balance, on_chain_nonce).unwrap();
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
        let tx1 = tx.inc_price().next();

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
    fn get_highest_consecutive_transaction_by_sender() {
        // Set up a mock transaction factory and a new transaction pool.
        let mut pool = TxPool::new(MockOrdering::default(), PoolConfig::default());
        let mut f = MockTransactionFactory::default();

        // Create transactions with nonces 0, 1, 2, 4, 5.
        let sender = Address::random();
        let txs: Vec<_> = vec![0, 1, 2, 4, 5, 8, 9];
        for nonce in txs {
            let mut mock_tx = MockTransaction::eip1559();
            mock_tx.set_sender(sender);
            mock_tx.set_nonce(nonce);

            let validated_tx = f.validated(mock_tx);
            pool.add_transaction(validated_tx, U256::from(1000), 0).unwrap();
        }

        // Get last consecutive transaction
        let sender_id = f.ids.sender_id(&sender).unwrap();
        let next_tx =
            pool.get_highest_consecutive_transaction_by_sender(sender_id.into_transaction_id(0));
        assert_eq!(next_tx.map(|tx| tx.nonce()), Some(2), "Expected nonce 2 for on-chain nonce 0");

        let next_tx =
            pool.get_highest_consecutive_transaction_by_sender(sender_id.into_transaction_id(4));
        assert_eq!(next_tx.map(|tx| tx.nonce()), Some(5), "Expected nonce 5 for on-chain nonce 4");

        let next_tx =
            pool.get_highest_consecutive_transaction_by_sender(sender_id.into_transaction_id(5));
        assert_eq!(next_tx.map(|tx| tx.nonce()), Some(5), "Expected nonce 5 for on-chain nonce 5");

        // update the tracked nonce
        let mut info = SenderInfo::default();
        info.update(8, U256::ZERO);
        pool.sender_info.insert(sender_id, info);
        let next_tx =
            pool.get_highest_consecutive_transaction_by_sender(sender_id.into_transaction_id(5));
        assert_eq!(next_tx.map(|tx| tx.nonce()), Some(9), "Expected nonce 9 for on-chain nonce 8");
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

        let mut changed_senders = HashMap::default();
        changed_senders.insert(
            id.sender,
            SenderInfo { state_nonce: next.nonce(), balance: U256::from(1_000) },
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
        let a_sender = address!("0x000000000000000000000000000000000000000a");

        // set the base fee of the pool
        let mut block_info = pool.block_info();
        block_info.pending_blob_fee = Some(100);
        block_info.pending_basefee = 100;

        // update
        pool.set_block_info(block_info);

        // 2 txs, that should put the pool over the size limit but not max txs
        let a_txs = MockTransactionSet::dependent(a_sender, 0, 2, TxType::Eip4844)
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
        let a_sender = address!("0x000000000000000000000000000000000000000a");

        // set the base fee of the pool
        let pool_base_fee = 100;
        pool.update_basefee(pool_base_fee);

        // 2 txs, that should put the pool over the size limit but not max txs
        let a_txs = MockTransactionSet::dependent(a_sender, 0, 3, TxType::Eip1559)
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
        let v0 = f.validated(tx_0);
        let v1 = f.validated(tx_1);
        let v2 = f.validated(tx_2);

        // Add first 2 to the pool
        let _res = pool.add_transaction(v0.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v1, on_chain_balance, on_chain_nonce).unwrap();

        assert!(pool.queued_transactions().is_empty());
        assert_eq!(2, pool.pending_transactions().len());

        // Remove first (nonce 0) - simulating that it was taken to be a part of the block.
        pool.prune_transaction_by_hash(v0.hash());

        // Now add transaction with nonce 2
        let _res = pool.add_transaction(v2, on_chain_balance, on_chain_nonce).unwrap();

        // v2 is in the queue now. v1 is still in 'pending'.
        assert_eq!(1, pool.queued_transactions().len());
        assert_eq!(1, pool.pending_transactions().len());

        // Simulate new block arrival - and chain nonce increasing.
        let mut updated_accounts = HashMap::default();
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
        let v0 = f.validated(tx_0);
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
    fn test_remove_transactions() {
        let on_chain_balance = U256::from(10_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx_0 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_1 = tx_0.next();
        let tx_2 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_3 = tx_2.next();

        // Create 4 transactions
        let v0 = f.validated(tx_0);
        let v1 = f.validated(tx_1);
        let v2 = f.validated(tx_2);
        let v3 = f.validated(tx_3);

        // Add them to the pool
        let _res = pool.add_transaction(v0.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v1.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v2.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v3.clone(), on_chain_balance, on_chain_nonce).unwrap();

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(4, pool.pending_transactions().len());

        pool.remove_transactions(vec![*v0.hash(), *v2.hash()]);

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(2, pool.pending_transactions().len());
        assert!(pool.contains(v1.hash()));
        assert!(pool.contains(v3.hash()));
    }

    #[test]
    fn test_remove_transactions_and_descendants() {
        let on_chain_balance = U256::from(10_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx_0 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_1 = tx_0.next();
        let tx_2 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_3 = tx_2.next();
        let tx_4 = tx_3.next();

        // Create 5 transactions
        let v0 = f.validated(tx_0);
        let v1 = f.validated(tx_1);
        let v2 = f.validated(tx_2);
        let v3 = f.validated(tx_3);
        let v4 = f.validated(tx_4);

        // Add them to the pool
        let _res = pool.add_transaction(v0.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v1, on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v2.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v3, on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v4, on_chain_balance, on_chain_nonce).unwrap();

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(5, pool.pending_transactions().len());

        pool.remove_transactions_and_descendants(vec![*v0.hash(), *v2.hash()]);

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(0, pool.pending_transactions().len());
    }
    #[test]
    fn test_remove_descendants() {
        let on_chain_balance = U256::from(10_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx_0 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_1 = tx_0.next();
        let tx_2 = tx_1.next();
        let tx_3 = tx_2.next();

        // Create 4 transactions
        let v0 = f.validated(tx_0);
        let v1 = f.validated(tx_1);
        let v2 = f.validated(tx_2);
        let v3 = f.validated(tx_3);

        // Add them to the pool
        let _res = pool.add_transaction(v0.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v1, on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v2, on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v3, on_chain_balance, on_chain_nonce).unwrap();

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(4, pool.pending_transactions().len());

        let mut removed = Vec::new();
        pool.remove_transaction(v0.id());
        pool.remove_descendants(v0.id(), &mut removed);

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(0, pool.pending_transactions().len());
        assert_eq!(3, removed.len());
    }
    #[test]
    fn test_remove_transactions_by_sender() {
        let on_chain_balance = U256::from(10_000);
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx_0 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_1 = tx_0.next();
        let tx_2 = MockTransaction::eip1559().set_gas_price(100).inc_limit();
        let tx_3 = tx_2.next();
        let tx_4 = tx_3.next();

        // Create 5 transactions
        let v0 = f.validated(tx_0);
        let v1 = f.validated(tx_1);
        let v2 = f.validated(tx_2);
        let v3 = f.validated(tx_3);
        let v4 = f.validated(tx_4);

        // Add them to the pool
        let _res = pool.add_transaction(v0.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v1.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v2.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v3, on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v4, on_chain_balance, on_chain_nonce).unwrap();

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(5, pool.pending_transactions().len());

        pool.remove_transactions_by_sender(v2.sender_id());

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(2, pool.pending_transactions().len());
        assert!(pool.contains(v0.hash()));
        assert!(pool.contains(v1.hash()));
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
        let v0 = f.validated(tx_0);
        let v1 = f.validated(tx_1);
        let v2 = f.validated(tx_2);
        let v3 = f.validated(tx_3);

        // Add first 2 to the pool
        let _res = pool.add_transaction(v0.clone(), on_chain_balance, on_chain_nonce).unwrap();
        let _res = pool.add_transaction(v1, on_chain_balance, on_chain_nonce).unwrap();

        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(2, pool.pending_transactions().len());

        // Remove first (nonce 0) - simulating that it was taken to be a part of the block.
        pool.remove_transaction(v0.id());

        // Now add transaction with nonce 2
        let _res = pool.add_transaction(v2, on_chain_balance, on_chain_nonce).unwrap();

        // v2 is in the queue now. v1 is still in 'pending'.
        assert_eq!(1, pool.queued_transactions().len());
        assert_eq!(1, pool.pending_transactions().len());

        // Simulate new block arrival - and chain nonce increasing.
        let mut updated_accounts = HashMap::default();
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
        let _res = pool.add_transaction(v3, on_chain_balance, on_chain_nonce).unwrap();
        assert_eq!(0, pool.queued_transactions().len());
        assert_eq!(3, pool.pending_transactions().len());

        // It should have returned transactions in order (v1, v2, v3 - as there is nothing blocking
        // them).
        assert_eq!(
            pool.best_transactions().map(|x| x.id().nonce).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn test_pending_ordering() {
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::new(MockOrdering::default(), Default::default());

        let tx_0 = MockTransaction::eip1559().with_nonce(1).set_gas_price(100).inc_limit();
        let tx_1 = tx_0.next();

        let v0 = f.validated(tx_0);
        let v1 = f.validated(tx_1);

        // nonce gap, tx should be queued
        pool.add_transaction(v0.clone(), U256::MAX, 0).unwrap();
        assert_eq!(1, pool.queued_transactions().len());

        // nonce gap is closed on-chain, both transactions should be moved to pending
        pool.add_transaction(v1, U256::MAX, 1).unwrap();

        assert_eq!(2, pool.pending_transactions().len());
        assert_eq!(0, pool.queued_transactions().len());

        assert_eq!(
            pool.pending_pool.independent().get(&v0.sender_id()).unwrap().transaction.nonce(),
            v0.nonce()
        );
    }

    // <https://github.com/paradigmxyz/reth/issues/12286>
    #[test]
    fn one_sender_one_independent_transaction() {
        let mut on_chain_balance = U256::from(4_999); // only enough for 4 txs
        let mut on_chain_nonce = 40;
        let mut f = MockTransactionFactory::default();
        let mut pool = TxPool::mock();
        let mut submitted_txs = Vec::new();

        // We use a "template" because we want all txs to have the same sender.
        let template =
            MockTransaction::eip1559().inc_price().inc_limit().with_value(U256::from(1_001));

        // Add 8 txs. Because the balance is only sufficient for 4, so the last 4 will be
        // Queued.
        for tx_nonce in 40..48 {
            let tx = f.validated(template.clone().with_nonce(tx_nonce).rng_hash());
            submitted_txs.push(*tx.id());
            pool.add_transaction(tx, on_chain_balance, on_chain_nonce).unwrap();
        }

        // A block is mined with two txs (so nonce is changed from 40 to 42).
        // Now the balance gets so high that it's enough to execute alltxs.
        on_chain_balance = U256::from(999_999);
        on_chain_nonce = 42;
        pool.remove_transaction(&submitted_txs[0]);
        pool.remove_transaction(&submitted_txs[1]);

        // Add 4 txs.
        for tx_nonce in 48..52 {
            pool.add_transaction(
                f.validated(template.clone().with_nonce(tx_nonce).rng_hash()),
                on_chain_balance,
                on_chain_nonce,
            )
            .unwrap();
        }

        let best_txs: Vec<_> = pool.pending().best().map(|tx| *tx.id()).collect();
        assert_eq!(best_txs.len(), 10); // 8 - 2 + 4 = 10

        assert_eq!(pool.pending_pool.independent().len(), 1);
    }
}
