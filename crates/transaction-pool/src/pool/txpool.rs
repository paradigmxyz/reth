//! The internal transaction pool implementation.
use crate::{
    config::MAX_ACCOUNT_SLOTS_PER_SENDER,
    error::PoolError,
    identifier::{SenderId, TransactionId},
    metrics::TxPoolMetrics,
    pool::{
        best::BestTransactions,
        parked::{BasefeeOrd, ParkedPool, QueuedOrd},
        pending::PendingPool,
        state::{SubPool, TxState},
        update::{Destination, PoolUpdate},
        AddedPendingTransaction, AddedTransaction, OnNewBlockOutcome,
    },
    traits::{PoolSize, StateDiff},
    OnNewBlockEvent, PoolConfig, PoolResult, PoolTransaction, TransactionOrdering,
    ValidPoolTransaction, U256,
};
use fnv::FnvHashMap;
use reth_primitives::{TxHash, H256};
use ruint::uint;
use std::{
    cmp::Ordering,
    collections::{btree_map::Entry, hash_map, BTreeMap, HashMap},
    fmt,
    ops::Bound::{Excluded, Unbounded},
    sync::Arc,
};

/// The minimal value the basefee can decrease to
///
/// The `BASE_FEE_MAX_CHANGE_DENOMINATOR` (https://eips.ethereum.org/EIPS/eip-1559) is `8`, or 12.5%, once the base fee has dropped to `7` WEI it cannot decrease further because 12.5% of 7 is less than 1.
pub(crate) const MIN_PROTOCOL_BASE_FEE: U256 = uint!(0x7_U256);

/// A pool that manages transactions.
///
/// This pool maintains the state of all transactions and stores them accordingly.

#[cfg_attr(doc, aquamarine::aquamarine)]
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
///     end
///   end
///   discard([discard])
///   production([Block Production])
///   new([New Block])
///   A[Incoming Tx] --> B[Validation] -->|insert| pool
///   pool --> |if ready| B1
///   pool --> |if ready + basfee too low| B2
///   pool --> |nonce gap or lack of funds| B3
///   pool --> |update| pool
///   B1 --> |best| production
///   B2 --> |worst| discard
///   B3 --> |worst| discard
///   B1 --> |increased fee| B2
///   B2 --> |decreased fee| B1
///   B3 --> |promote| B1
///   B3 -->  |promote| B2
///   new -->  |apply state changes| pool
/// ```
pub struct TxPool<T: TransactionOrdering> {
    /// Contains the currently known info
    sender_info: FnvHashMap<SenderId, SenderInfo>,
    /// pending subpool
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
    /// All transactions in the pool.
    all_transactions: AllTransactions<T::Transaction>,
    /// Transaction pool metrics
    metrics: TxPoolMetrics,
}

// === impl TxPool ===

impl<T: TransactionOrdering> TxPool<T> {
    /// Create a new graph pool instance.
    pub(crate) fn new(ordering: Arc<T>, config: PoolConfig) -> Self {
        Self {
            sender_info: Default::default(),
            pending_pool: PendingPool::new(ordering),
            queued_pool: Default::default(),
            basefee_pool: Default::default(),
            all_transactions: AllTransactions::new(config.max_account_slots),
            config,
            metrics: Default::default(),
        }
    }

    /// Returns access to the [`AllTransactions`] container.
    pub(crate) fn all(&self) -> &AllTransactions<T::Transaction> {
        &self.all_transactions
    }

    /// Returns stats about the size of pool.
    pub(crate) fn size(&self) -> PoolSize {
        PoolSize {
            pending: self.pending_pool.len(),
            pending_size: self.pending_pool.size(),
            basefee: self.basefee_pool.len(),
            basefee_size: self.basefee_pool.size(),
            queued: self.queued_pool.len(),
            queued_size: self.queued_pool.size(),
        }
    }

    /// Updates the pool based on the changed base fee.
    ///
    /// This enforces the dynamic fee requirement.
    pub(crate) fn update_base_fee(&mut self, _new_base_fee: U256) {
        // TODO update according to the changed base_fee
        todo!()
    }

    /// Returns an iterator that yields transactions that are ready to be included in the block.
    pub(crate) fn best_transactions(&self) -> BestTransactions<T> {
        self.pending_pool.best()
    }

    /// Returns if the transaction for the given hash is already included in this pool
    pub(crate) fn contains(&self, tx_hash: &TxHash) -> bool {
        self.all_transactions.contains(tx_hash)
    }

    /// Returns the transaction for the given hash.
    pub(crate) fn get(
        &self,
        tx_hash: &TxHash,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        self.all_transactions.by_hash.get(tx_hash).cloned()
    }

    /// Returns all transaction for the hashes, if it exists.
    pub(crate) fn get_all<'a>(
        &'a self,
        txs: impl IntoIterator<Item = TxHash> + 'a,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + 'a {
        txs.into_iter().filter_map(|tx| self.get(&tx))
    }

    /// Updates the entire pool after a new block was mined.
    ///
    /// This removes all mined transactions, updates according to the new base fee and rechecks
    /// sender allowance.
    pub(crate) fn on_new_block(&mut self, event: OnNewBlockEvent) -> OnNewBlockOutcome {
        // Remove all transaction that were included in the block
        for tx_hash in &event.mined_transactions {
            self.remove_transaction_by_hash(tx_hash);
            // Update removed transactions metric
            self.metrics.removed_transactions.increment(1);
        }

        // Apply the state changes to the total set of transactions which triggers sub-pool updates.
        let updates =
            self.all_transactions.update(event.pending_block_base_fee, &event.state_changes);

        // Process the sub-pool updates
        let UpdateOutcome { promoted, discarded, .. } = self.process_updates(updates);

        OnNewBlockOutcome {
            block_hash: event.hash,
            mined: event.mined_transactions,
            promoted,
            discarded,
        }
    }

    /// Adds the transaction into the pool.
    ///
    /// This pool consists of two three-pools: `Queued`, `Pending` and `BaseFee`.
    ///
    /// The `Queued` pool contains transactions with gaps in its dependency tree: It requires
    /// additional transaction that are note yet present in the pool. And transactions that the
    /// sender can not afford with the current balance.
    ///
    /// The `Pending` pool contains all transactions that have no nonce gaps, and can be afforded by
    /// the sender. It only contains transactions that are ready to be included in the pending
    /// block. In the pending pool are all transactions that could be listed currently, but not
    /// necessarily independently. However, this pool never contains transactions with nonce gaps. A
    /// transaction is considered `ready` when it has the lowest nonce of all transactions from the
    /// same sender. Which is equals to the chain nonce of the sender in the pending pool.
    ///
    /// The `BaseFee` pool contains transaction that currently can't satisfy the dynamic fee
    /// requirement. With EIP-1559, transactions can become executable or not without any changes to
    /// the sender's balance or nonce and instead their feeCap determines whether the
    /// transaction is _currently_ (on the current state) ready or needs to be parked until the
    /// feeCap satisfies the block's baseFee.
    pub(crate) fn add_transaction(
        &mut self,
        tx: ValidPoolTransaction<T::Transaction>,
        on_chain_balance: U256,
        on_chain_nonce: u64,
    ) -> PoolResult<AddedTransaction<T::Transaction>> {
        // Update sender info with balance and nonce
        self.sender_info
            .entry(tx.sender_id())
            .or_default()
            .update(on_chain_nonce, on_chain_balance);

        let _hash = *tx.hash();

        match self.all_transactions.insert_tx(tx, on_chain_balance, on_chain_nonce) {
            Ok(InsertOk { transaction, move_to, replaced_tx, updates, .. }) => {
                self.add_new_transaction(transaction.clone(), replaced_tx, move_to);
                // Update inserted transactions metric
                self.metrics.inserted_transactions.increment(1);
                let UpdateOutcome { promoted, discarded, removed } = self.process_updates(updates);

                // This transaction was moved to the pending pool.
                let res = if move_to.is_pending() {
                    AddedTransaction::Pending(AddedPendingTransaction {
                        transaction,
                        promoted,
                        discarded,
                        removed,
                    })
                } else {
                    AddedTransaction::Parked { transaction, subpool: move_to }
                };

                Ok(res)
            }
            Err(e) => {
                // Update invalid transactions metric
                self.metrics.invalid_transactions.increment(1);
                match e {
                    InsertErr::Underpriced { existing, .. } => {
                        Err(PoolError::ReplacementUnderpriced(existing))
                    }
                    InsertErr::ProtocolFeeCapTooLow { transaction, fee_cap } => {
                        Err(PoolError::ProtocolFeeCapTooLow(*transaction.hash(), fee_cap))
                    }
                    InsertErr::ExceededSenderTransactionsCapacity { transaction } => {
                        Err(PoolError::SpammerExceededCapacity(
                            transaction.sender(),
                            *transaction.hash(),
                        ))
                    }
                }
            }
        }
    }

    /// Maintenance task to apply a series of updates.
    ///
    /// This will move/discard the given transaction according to the `PoolUpdate`
    fn process_updates(
        &mut self,
        updates: impl IntoIterator<Item = PoolUpdate>,
    ) -> UpdateOutcome<T::Transaction> {
        let mut outcome = UpdateOutcome::default();
        for update in updates {
            let PoolUpdate { id, hash, current, destination } = update;
            match destination {
                Destination::Discard => {
                    outcome.discarded.push(hash);
                }
                Destination::Pool(move_to) => {
                    debug_assert!(!move_to.eq(&current), "destination must be different");
                    self.move_transaction(current, move_to, &id);
                }
            }
        }
        outcome
    }

    /// Moves a transaction from one sub pool to another.
    ///
    /// This will remove the given transaction from one sub-pool and insert it in the other
    /// sub-pool.
    fn move_transaction(&mut self, from: SubPool, to: SubPool, id: &TransactionId) {
        if let Some(tx) = self.remove_from_subpool(from, id) {
            self.add_transaction_to_pool(to, tx);
        }
    }

    /// Removes and returns all matching transactions from the pool.
    pub(crate) fn remove_invalid(
        &mut self,
        hashes: impl IntoIterator<Item = TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        hashes.into_iter().filter_map(|hash| self.remove_transaction_by_hash(&hash)).collect()
    }

    /// Remove the transaction from the entire pool.
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
        tx_hash: &H256,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let (tx, pool) = self.all_transactions.remove_transaction_by_hash(tx_hash)?;
        self.remove_from_subpool(pool, tx.id())
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
        }
    }

    /// Removes _only_ the descendants of the given transaction from the entire pool.
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

    /// Removes the transaction from the given pool
    fn add_transaction_to_pool(
        &mut self,
        pool: SubPool,
        tx: Arc<ValidPoolTransaction<T::Transaction>>,
    ) {
        match pool {
            SubPool::Queued => {
                self.queued_pool.add_transaction(tx);
            }
            SubPool::Pending => {
                self.pending_pool.add_transaction(tx);
            }
            SubPool::BaseFee => {
                self.basefee_pool.add_transaction(tx);
            }
        }
    }

    /// Inserts the transaction into the given sub-pool
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

        self.add_transaction_to_pool(pool, transaction)
    }

    /// Ensures that the transactions in the sub-pools are within the given bounds.
    ///
    /// If the current size exceeds the given bounds, the worst transactions are evicted from the
    /// pool and returned.
    pub(crate) fn discard_worst(&mut self) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = Vec::new();

        // Helper macro that discards the worst transactions for the pools
        macro_rules! discard_worst {
            ($this:ident, $removed:ident,  [$($limit:ident => $pool:ident),*]  ) => {
                $ (
                while $this
                        .config
                        .$limit
                        .is_exceeded($this.$pool.len(), $this.$pool.size())
                    {
                        if let Some(tx) = $this.$pool.pop_worst() {
                            let id = tx.transaction_id;
                            removed.push(tx);
                            $this.remove_descendants(&id, &mut $removed);
                        }
                    }

                )*
            };
        }

        discard_worst!(
            self, removed, [
                pending_limit  => pending_pool,
                basefee_limit  => basefee_pool,
                queued_limit  => queued_pool
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
}

// Additional test impls
#[cfg(any(test, feature = "test-utils"))]
#[allow(missing_docs)]
impl<T: TransactionOrdering> TxPool<T> {
    pub(crate) fn pending(&self) -> &PendingPool<T> {
        &self.pending_pool
    }

    pub(crate) fn base_fee(&self) -> &ParkedPool<BasefeeOrd<T::Transaction>> {
        &self.basefee_pool
    }

    pub(crate) fn queued(&self) -> &ParkedPool<QueuedOrd<T::Transaction>> {
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
    /// Expected base fee for the pending block.
    pending_basefee: U256,
    /// Minimum base fee required by the protocol.
    ///
    /// Transactions with a lower base fee will never be included by the chain
    minimal_protocol_basefee: U256,
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
}

impl<T: PoolTransaction> AllTransactions<T> {
    /// Create a new instance
    fn new(max_account_slots: usize) -> Self {
        Self { max_account_slots, ..Default::default() }
    }

    /// Returns an iterator over all _unique_ hashes in the pool
    pub(crate) fn hashes_iter(&self) -> impl Iterator<Item = TxHash> + '_ {
        self.by_hash.keys().copied()
    }

    /// Returns if the transaction for the given hash is already included in this pool
    pub(crate) fn contains(&self, tx_hash: &TxHash) -> bool {
        self.by_hash.contains_key(tx_hash)
    }

    /// Returns the internal transaction with additional metadata
    #[cfg(test)]
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
        pending_block_base_fee: U256,
        _state_diffs: &StateDiff,
    ) -> Vec<PoolUpdate> {
        // update new basefee
        self.pending_basefee = pending_block_base_fee;

        // TODO(mattsse): probably good idea to allocate some capacity here.
        let mut updates = Vec::new();

        let mut iter = self.txs.iter_mut().peekable();

        // Loop over all individual senders and update all affected transactions.
        // One sender may have up to `max_account_slots` transactions here, which means, worst case
        // `max_accounts_slots` need to be updated, for example if the first transaction is blocked
        // due to too low base fee.
        // However, we don't have to necessarily check every transaction of a sender. If no updates
        // are possible (nonce gap) then we can skip to the next sender.

        // The `unique_sender` loop will process the first transaction of all senders, update its
        // state and internally update all consecutive transactions
        'unique_sender: while let Some((id, tx)) = iter.next() {
            // Advances the iterator to the next sender
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
            // If there's a nonce gap, we can shortcircuit, because there's nothing to update.
            if tx.state.has_nonce_gap() {
                next_sender!(iter);
                continue
            }

            // TODO(mattsse): if account has balance changes or mined transactions the balance needs
            // to be checked here

            // Since this is the first transaction of the sender, it has no parked ancestors
            tx.state.insert(TxState::NO_PARKED_ANCESTORS);

            // Update the first transaction of this sender.
            Self::update_base_fee(&pending_block_base_fee, tx);
            // Track if the transaction's sub-pool changed.
            Self::record_subpool_update(&mut updates, tx);

            // Track blocking transactions.
            let mut has_parked_ancestor = !tx.state.is_pending();

            // Update all consecutive transaction of this sender
            while let Some((peek, ref mut tx)) = iter.peek_mut() {
                if peek.sender != id.sender {
                    // Found the next sender
                    continue 'unique_sender
                }

                if tx.state.has_nonce_gap() {
                    next_sender!(iter);
                    continue 'unique_sender
                }

                // Update ancestor condition.
                if has_parked_ancestor {
                    tx.state.remove(TxState::NO_PARKED_ANCESTORS);
                } else {
                    tx.state.insert(TxState::NO_PARKED_ANCESTORS);
                }
                has_parked_ancestor = !tx.state.is_pending();

                // Update and record sub-pool changes.
                Self::update_base_fee(&pending_block_base_fee, tx);
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
    fn update_base_fee(pending_block_base_fee: &U256, tx: &mut PoolInternalTransaction<T>) {
        // Recheck dynamic fee condition.
        if let Some(fee_cap) = tx.transaction.max_fee_per_gas() {
            match fee_cap.cmp(pending_block_base_fee) {
                Ordering::Greater | Ordering::Equal => {
                    tx.state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
                }
                Ordering::Less => {
                    tx.state.remove(TxState::ENOUGH_FEE_CAP_BLOCK);
                }
            }
        }
    }

    /// Returns an iterator over all transactions for the given sender, starting with the lowest
    /// nonce
    #[cfg(test)]
    #[allow(unused)]
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
    #[allow(unused)]
    pub(crate) fn txs_iter_mut(
        &mut self,
        sender: SenderId,
    ) -> impl Iterator<Item = (&TransactionId, &mut PoolInternalTransaction<T>)> + '_ {
        self.txs
            .range_mut((sender.start_bound(), Unbounded))
            .take_while(move |(other, _)| sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id but have the same sender.
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
    /// NOTE: The range is _exclusive_
    pub(crate) fn descendant_txs_exclusive_mut<'a, 'b: 'a>(
        &'a mut self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a mut PoolInternalTransaction<T>)> + '_ {
        self.txs
            .range_mut((Excluded(id), Unbounded))
            .take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id but have the same sender.
    ///
    /// NOTE: The range is _inclusive_: if the transaction that belongs to `id` it field be the
    /// first value.
    pub(crate) fn descendant_txs<'a, 'b: 'a>(
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
        tx_hash: &H256,
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

    /// Additional checks for a new transaction.
    ///
    /// This will enforce all additional rules in the context of this pool, such as:
    ///   - Spam protection: reject new non-local transaction from a sender that exhausted its slot
    ///     capacity.
    fn ensure_valid(
        &self,
        transaction: ValidPoolTransaction<T>,
    ) -> Result<ValidPoolTransaction<T>, InsertErr<T>> {
        if !transaction.origin.is_local() {
            let current_txs =
                self.tx_counter.get(&transaction.sender_id()).copied().unwrap_or_default();
            if current_txs >= self.max_account_slots {
                return Err(InsertErr::ExceededSenderTransactionsCapacity {
                    transaction: Arc::new(transaction),
                })
            }
        }
        Ok(transaction)
    }

    /// Inserts a new transaction into the pool.
    ///
    /// If the transaction already exists, it will be replaced if not underpriced.
    /// Returns info to which sub-pool the transaction should be moved.
    /// Also returns a set of pool updates triggered by this insert, that need to be handled by the
    /// caller.
    ///
    /// These can include:
    ///      - closing nonce gaps of descendant transactions
    ///      - enough balance updates
    pub(crate) fn insert_tx(
        &mut self,
        transaction: ValidPoolTransaction<T>,
        on_chain_balance: U256,
        on_chain_nonce: u64,
    ) -> InsertResult<T> {
        assert!(on_chain_nonce <= transaction.nonce(), "Invalid transaction");

        let transaction = Arc::new(self.ensure_valid(transaction)?);
        let tx_id = *transaction.id();
        let mut state = TxState::default();
        let mut cumulative_cost = U256::ZERO;
        let mut updates = Vec::new();

        let predecessor =
            TransactionId::ancestor(transaction.transaction.nonce(), on_chain_nonce, tx_id.sender);

        // If there's no predecessor then this is the next transaction.
        if predecessor.is_none() {
            state.insert(TxState::NO_NONCE_GAPS);
            state.insert(TxState::NO_PARKED_ANCESTORS);
        }

        // Check dynamic fee
        if let Some(fee_cap) = transaction.max_fee_per_gas() {
            if fee_cap < self.minimal_protocol_basefee {
                return Err(InsertErr::ProtocolFeeCapTooLow { transaction, fee_cap })
            }
            if fee_cap >= self.pending_basefee {
                state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
            }
        } else {
            // legacy transactions always satisfy the condition
            state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
        }

        // Ensure tx does not exceed block gas limit
        if transaction.gas_limit() < self.block_gas_limit {
            state.insert(TxState::NOT_TOO_MUCH_GAS);
        }

        let mut replaced_tx = None;

        let pool_tx = PoolInternalTransaction {
            transaction: transaction.clone(),
            subpool: SubPool::Queued,
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
                // Transaction already exists
                // Ensure the new transaction is not underpriced
                if transaction.is_underpriced(entry.get().transaction.as_ref()) {
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
            let mut descendants = self.descendant_txs_mut(&on_chain_id).peekable();

            // Tracks the next nonce we expect if the transactions are gapless
            let mut next_nonce = on_chain_id.nonce;

            // We need to find out if the next transaction of the sender is considered pending
            //
            let mut has_parked_ancestor = if predecessor.is_none() {
                // the new transaction is the next one
                false
            } else {
                // SAFETY: the transaction was added above so the _inclusive_ descendants iterator
                // returns at least 1 tx.
                let (id, tx) = descendants.peek().expect("Includes >= 1; qed.");
                if id.nonce < tx_id.nonce {
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

                if tx_id.eq(id) {
                    // if it is the new transaction, track the state
                    state = tx.state;
                } else {
                    tx.subpool = tx.state.into();
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
            self.tx_inc(tx_id.sender);
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
}

#[cfg(test)]
#[allow(missing_docs)]
impl<T: PoolTransaction> AllTransactions<T> {
    pub(crate) fn tx_count(&self, sender: SenderId) -> usize {
        self.tx_counter.get(&sender).copied().unwrap_or_default()
    }
}

impl<T: PoolTransaction> Default for AllTransactions<T> {
    fn default() -> Self {
        Self {
            max_account_slots: MAX_ACCOUNT_SLOTS_PER_SENDER,
            pending_basefee: Default::default(),
            minimal_protocol_basefee: MIN_PROTOCOL_BASE_FEE,
            block_gas_limit: 30_000_000,
            by_hash: Default::default(),
            txs: Default::default(),
            tx_counter: Default::default(),
        }
    }
}

/// Result type for inserting a transaction
pub(crate) type InsertResult<T> = Result<InsertOk<T>, InsertErr<T>>;

/// Err variant of `InsertResult`
#[derive(Debug)]
pub(crate) enum InsertErr<T: PoolTransaction> {
    /// Attempted to replace existing transaction, but was underpriced
    Underpriced { transaction: Arc<ValidPoolTransaction<T>>, existing: TxHash },
    /// The transactions feeCap is lower than the chain's minimum fee requirement.
    ///
    /// See also [`MIN_PROTOCOL_BASE_FEE`]
    ProtocolFeeCapTooLow { transaction: Arc<ValidPoolTransaction<T>>, fee_cap: U256 },
    /// Sender currently exceeds the configured limit for max account slots.
    ///
    /// The sender can be considered a spammer at this point.
    ExceededSenderTransactionsCapacity { transaction: Arc<ValidPoolTransaction<T>> },
}

/// Transaction was successfully inserted into the pool
#[derive(Debug)]
pub(crate) struct InsertOk<T: PoolTransaction> {
    /// Ref to the inserted transaction.
    transaction: Arc<ValidPoolTransaction<T>>,
    /// Where to move the transaction to.
    move_to: SubPool,
    /// Current state of the inserted tx.
    state: TxState,
    /// The transaction that was replaced by this.
    replaced_tx: Option<(Arc<ValidPoolTransaction<T>>, SubPool)>,
    /// Additional updates to transactions affected by this change.
    updates: Vec<PoolUpdate>,
}

/// The internal transaction typed used by `AllTransactions` which also additional info used for
/// determining the current state of the transaction.
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
        self.cumulative_cost + self.transaction.cost
    }
}

/// Tracks the result after updating the pool
#[derive(Debug)]
pub struct UpdateOutcome<T: PoolTransaction> {
    /// transactions promoted to the ready queue
    promoted: Vec<TxHash>,
    /// transaction that failed and became discarded
    discarded: Vec<TxHash>,
    /// Transactions removed from the Ready pool
    removed: Vec<Arc<ValidPoolTransaction<T>>>,
}

impl<T: PoolTransaction> Default for UpdateOutcome<T> {
    fn default() -> Self {
        Self { promoted: vec![], discarded: vec![], removed: vec![] }
    }
}

/// Represents the outcome of a prune
pub struct PruneResult<T: PoolTransaction> {
    /// A list of added transactions that a pruned marker satisfied
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

/// Stores relevant context about a sender.
#[derive(Debug, Clone, Default)]
struct SenderInfo {
    /// current nonce of the sender.
    state_nonce: u64,
    /// Balance of the sender at the current point.
    balance: U256,
}

// === impl SenderInfo ===

impl SenderInfo {
    /// Creates a new entry for an incoming, not yet tracked sender.
    fn new_incoming(state_nonce: u64, balance: U256) -> Self {
        Self { state_nonce, balance }
    }

    /// Updates the info with the new values.
    fn update(&mut self, state_nonce: u64, balance: U256) {
        *self = Self { state_nonce, balance };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{MockTransaction, MockTransactionFactory},
        traits::TransactionOrigin,
    };

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
        assert!(res.is_err());
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
    fn insert_replace() {
        let on_chain_balance = U256::ZERO;
        let on_chain_nonce = 0;
        let mut f = MockTransactionFactory::default();
        let mut pool = AllTransactions::default();
        let tx = MockTransaction::eip1559().inc_price().inc_limit();
        let first = f.validated(tx.clone());
        let _res = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce);
        let replacement = f.validated(tx.rng_hash().inc_price());
        let InsertOk { updates, replaced_tx, .. } =
            pool.insert_tx(replacement.clone(), on_chain_balance, on_chain_nonce).unwrap();
        assert!(updates.is_empty());
        let replaced = replaced_tx.unwrap();
        assert_eq!(replaced.0.hash(), first.hash());

        assert!(!pool.contains(first.hash()));
        assert!(pool.contains(replacement.hash()));
        assert_eq!(pool.len(), 1);
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
        let tx =
            MockTransaction::eip1559().inc_nonce().set_gas_price(U256::from(100u64)).inc_limit();
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
        pool.pending_basefee = pool.minimal_protocol_basefee.checked_add(U256::from(1)).unwrap();
        let tx = MockTransaction::eip1559().inc_nonce().inc_limit();
        let first = f.validated(tx.clone());

        let _res = pool.insert_tx(first.clone(), on_chain_balance, on_chain_nonce);

        let first_in_pool = pool.get(first.id()).unwrap();

        assert!(tx.get_gas_price() < pool.pending_basefee);
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
}
