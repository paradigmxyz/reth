use crate::{
    config::{LocalTransactionConfig, TXPOOL_MAX_ACCOUNT_SLOTS_PER_SENDER},
    identifier::{SenderId, TransactionId},
    metrics::AllTransactionsMetrics,
    pool::{
        state::{SubPool, TxState},
        update::{Destination, PoolUpdate},
    },
    traits::BlockInfo,
    PoolConfig, PoolTransaction, PriceBumpConfig, ValidPoolTransaction, U256,
};
use alloy_eips::{
    eip1559::{ETHEREUM_BLOCK_GAS_LIMIT_30M, MIN_PROTOCOL_BASE_FEE},
    eip4844::BLOB_TX_MIN_BLOB_GASPRICE,
};
use alloy_primitives::{TxHash, B256};
use rustc_hash::FxHashMap;
use std::{
    cmp::Ordering,
    collections::{btree_map::Entry, hash_map, BTreeMap, HashMap},
    ops::Bound::{Excluded, Unbounded},
    sync::Arc,
};

/// Container for _all_ transaction in the pool.
///
/// This is the sole entrypoint that's guarding all sub-pools, all sub-pool actions are always
/// derived from this set. Updates returned from this type must be applied to the sub-pools.
pub(crate) struct AllTransactions<T: PoolTransaction> {
    /// Minimum base fee required by the protocol.
    ///
    /// Transactions with a lower base fee will never be included by the chain
    pub(crate) minimal_protocol_basefee: u64,
    /// The max gas limit of the block
    pub(crate) block_gas_limit: u64,
    /// Max number of executable transaction slots guaranteed per account
    pub(crate) max_account_slots: usize,
    /// _All_ transactions identified by their hash.
    pub(crate) by_hash: HashMap<TxHash, Arc<ValidPoolTransaction<T>>>,
    /// _All_ transaction in the pool sorted by their sender and nonce pair.
    pub(crate) txs: BTreeMap<TransactionId, PoolInternalTransaction<T>>,
    /// Tracks the number of transactions by sender that are currently in the pool.
    pub(crate) tx_counter: FxHashMap<SenderId, usize>,
    /// The current block number the pool keeps track of.
    pub(crate) last_seen_block_number: u64,
    /// The current block hash the pool keeps track of.
    pub(crate) last_seen_block_hash: B256,
    /// Expected blob and base fee for the pending block.
    pub(crate) pending_fees: PendingFees,
    /// Configured price bump settings for replacements
    pub(crate) price_bumps: PriceBumpConfig,
    /// How to handle [`TransactionOrigin::Local`](crate::TransactionOrigin) transactions.
    pub(crate) local_transactions_config: LocalTransactionConfig,
    /// All Transactions metrics
    pub(crate) metrics: AllTransactionsMetrics,
}

impl<T: PoolTransaction> AllTransactions<T> {
    /// Create a new instance
    pub(crate) fn new(config: &PoolConfig) -> Self {
        Self {
            max_account_slots: config.max_account_slots,
            price_bumps: config.price_bumps,
            local_transactions_config: config.local_transactions_config.clone(),
            minimal_protocol_basefee: config.minimal_protocol_basefee,
            block_gas_limit: config.gas_limit,
            ..Default::default()
        }
    }

    /// Returns an iterator over all _unique_ hashes in the pool
    #[expect(dead_code)]
    pub(crate) fn hashes_iter(&self) -> impl Iterator<Item = TxHash> + '_ {
        self.by_hash.keys().copied()
    }

    /// Returns an iterator over all transactions in the pool
    pub(crate) fn transactions_iter(
        &self,
    ) -> impl Iterator<Item = &Arc<ValidPoolTransaction<T>>> + '_ {
        self.by_hash.values()
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
        self.metrics.all_transactions_by_all_senders.increment(1.0);
    }

    /// Decrements the transaction counter for the sender
    pub(crate) fn tx_decr(&mut self, sender: SenderId) {
        if let hash_map::Entry::Occupied(mut entry) = self.tx_counter.entry(sender) {
            let count = entry.get_mut();
            if *count == 1 {
                entry.remove();
                self.metrics.all_transactions_by_all_senders.decrement(1.0);
                return
            }
            *count -= 1;
            self.metrics.all_transactions_by_all_senders.decrement(1.0);
        }
    }

    /// Updates the block specific info
    pub(crate) fn set_block_info(&mut self, block_info: BlockInfo) {
        let BlockInfo {
            block_gas_limit,
            last_seen_block_hash,
            last_seen_block_number,
            pending_basefee,
            pending_blob_fee,
        } = block_info;
        self.last_seen_block_number = last_seen_block_number;
        self.last_seen_block_hash = last_seen_block_hash;

        self.pending_fees.base_fee = pending_basefee;
        self.metrics.base_fee.set(pending_basefee as f64);

        self.block_gas_limit = block_gas_limit;

        if let Some(pending_blob_fee) = pending_blob_fee {
            self.pending_fees.blob_fee = pending_blob_fee;
            self.metrics.blob_base_fee.set(pending_blob_fee as f64);
        }
    }

    /// Updates the size metrics
    pub(crate) fn update_size_metrics(&self) {
        self.metrics.all_transactions_by_hash.set(self.by_hash.len() as f64);
        self.metrics.all_transactions_by_id.set(self.txs.len() as f64);
    }

    /// Rechecks all transactions in the pool against the changes.
    ///
    /// Possible changes are:
    ///
    /// For all transactions:
    ///   - decreased basefee: promotes from `basefee` to `pending` sub-pool.
    ///   - increased basefee: demotes from `pending` to `basefee` sub-pool.
    ///
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
        changed_accounts: &FxHashMap<SenderId, SenderInfo>,
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
                    if tx.transaction.cost() > &info.balance {
                        // sender lacks sufficient funds to pay for this transaction
                        tx.state.remove(TxState::ENOUGH_BALANCE);
                    } else {
                        tx.state.insert(TxState::ENOUGH_BALANCE);
                    }
                }

                changed_balance = Some(&info.balance);
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
                    if &cumulative_cost > changed_balance {
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
                destination: tx.subpool.into(),
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
    #[expect(dead_code)]
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
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + 'a {
        self.txs.range((Excluded(id), Unbounded)).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id but have the same sender.
    ///
    /// NOTE: The range is _inclusive_: if the transaction that belongs to `id` it will be the
    /// first value.
    pub(crate) fn descendant_txs_inclusive<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + 'a {
        self.txs.range(id..).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all mutable transactions that _follow_ after the given id but have the same sender.
    ///
    /// NOTE: The range is _inclusive_: if the transaction that belongs to `id` it field be the
    /// first value.
    pub(crate) fn descendant_txs_mut<'a, 'b: 'a>(
        &'a mut self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a mut PoolInternalTransaction<T>)> + 'a {
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
        self.update_size_metrics();
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

        let result =
            self.by_hash.remove(internal.transaction.hash()).map(|tx| (tx, internal.subpool));

        self.update_size_metrics();

        result
    }

    /// Checks if the given transaction's type conflicts with an existing transaction.
    ///
    /// See also [`ValidPoolTransaction::tx_type_conflicts_with`].
    ///
    /// Caution: This assumes that mutually exclusive invariant is always true for the same sender.
    #[inline]
    fn contains_conflicting_transaction(&self, tx: &ValidPoolTransaction<T>) -> bool {
        self.txs_iter(tx.transaction_id.sender)
            .next()
            .is_some_and(|(_, existing)| tx.tx_type_conflicts_with(&existing.transaction))
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
        on_chain_nonce: u64,
    ) -> Result<ValidPoolTransaction<T>, InsertErr<T>> {
        if !self.local_transactions_config.is_local(transaction.origin, transaction.sender_ref()) {
            let current_txs =
                self.tx_counter.get(&transaction.sender_id()).copied().unwrap_or_default();

            // Reject transactions if sender's capacity is exceeded.
            // If transaction's nonce matches on-chain nonce always let it through
            if current_txs >= self.max_account_slots && transaction.nonce() > on_chain_nonce {
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
                self.metrics.blob_transactions_nonce_gaps.increment(1);
                return Err(InsertErr::BlobTxHasNonceGap { transaction: Arc::new(new_blob_tx) })
            };
            if ancestor_tx.state.has_nonce_gap() {
                // the ancestor transaction already has a nonce gap, so we can't insert the new
                // blob
                self.metrics.blob_transactions_nonce_gaps.increment(1);
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
        } else if new_blob_tx.cost() > &on_chain_balance {
            // the transaction would go into overdraft
            return Err(InsertErr::Overdraft { transaction: Arc::new(new_blob_tx) })
        }

        Ok(new_blob_tx)
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
    ///  - If the pool already includes a blob transaction from the `transaction`'s sender, then the
    ///    `transaction` must also be a blob transaction
    ///  - If the pool already includes a non-blob transaction from the `transaction`'s sender, then
    ///    the `transaction` must _not_ be a blob transaction.
    ///
    /// In other words, the presence of blob transactions exclude non-blob transactions and vice
    /// versa.
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

        let mut transaction = self.ensure_valid(transaction, on_chain_nonce)?;

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
                if existing_transaction.is_underpriced(maybe_replacement, &self.price_bumps) {
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
            // Tracks the next nonce we expect if the transactions are gapless
            let mut next_nonce = on_chain_id.nonce;

            // We need to find out if the next transaction of the sender is considered pending
            // The direct descendant has _no_ parked ancestors because the `on_chain_nonce` is
            // pending, so we can set this to `false`
            let mut has_parked_ancestor = false;

            // Traverse all future transactions of the sender starting with the on chain nonce, and
            // update existing transactions: `[on_chain_nonce,..]`
            for (id, tx) in self.descendant_txs_mut(&on_chain_id) {
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
                            destination: tx.subpool.into(),
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

        self.update_size_metrics();

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
            block_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT_30M,
            by_hash: Default::default(),
            txs: Default::default(),
            tx_counter: Default::default(),
            last_seen_block_number: Default::default(),
            last_seen_block_hash: Default::default(),
            pending_fees: Default::default(),
            price_bumps: Default::default(),
            local_transactions_config: Default::default(),
            metrics: Default::default(),
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
        Self { base_fee: Default::default(), blob_fee: BLOB_TX_MIN_BLOB_GASPRICE }
    }
}

/// The internal transaction typed used by `AllTransactions` which also additional info used for
/// determining the current state of the transaction.
#[derive(Debug)]
pub(crate) struct PoolInternalTransaction<T: PoolTransaction> {
    /// The actual transaction object.
    pub(crate) transaction: Arc<ValidPoolTransaction<T>>,
    /// The `SubPool` that currently contains this transaction.
    pub(crate) subpool: SubPool,
    /// Keeps track of the current state of the transaction and therefore in which subpool it
    /// should reside
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
    pub(crate) fn update(&mut self, state_nonce: u64, balance: U256) {
        *self = Self { state_nonce, balance };
    }
}

/// Result type for inserting a transaction
pub(crate) type InsertResult<T> = Result<InsertOk<T>, InsertErr<T>>;

/// Err variant of `InsertResult`
#[derive(Debug)]
pub(crate) enum InsertErr<T: PoolTransaction> {
    /// Attempted to replace existing transaction, but was underpriced
    Underpriced {
        transaction: Arc<ValidPoolTransaction<T>>,
        #[expect(dead_code)]
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
    pub(crate) transaction: Arc<ValidPoolTransaction<T>>,
    /// Where to move the transaction to.
    pub(crate) move_to: SubPool,
    /// Current state of the inserted tx.
    #[allow(dead_code)]
    pub(crate) state: TxState,
    /// The transaction that was replaced by this.
    pub(crate) replaced_tx: Option<(Arc<ValidPoolTransaction<T>>, SubPool)>,
    /// Additional updates to transactions affected by this change.
    pub(crate) updates: Vec<PoolUpdate>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        pool::all_tx::{AllTransactions, InsertErr, InsertOk},
        test_utils::{MockTransaction, MockTransactionFactory},
        traits::TransactionOrigin,
    };
    use alloy_consensus::Transaction;

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
        let mut tx = MockTransaction::eip1559().inc_price().inc_limit();
        tx.set_priority_fee(100);
        tx.set_max_fee(100);
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

        // should also fail if the bump in max fee is not enough
        replacement.transaction.set_priority_fee(110);
        replacement.transaction.set_max_fee(109);
        let err =
            pool.insert_tx(replacement.clone(), on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::Underpriced { .. }));
        assert!(pool.contains(first.hash()));
        assert_eq!(pool.len(), 1);

        // should also fail if the bump in priority fee is not enough
        replacement.transaction.set_priority_fee(109);
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
        let tx = MockTransaction::eip4844().set_sender(tx.sender()).inc_price_by(100).inc_limit();
        let blob = f.validated(tx);
        let err = pool.insert_tx(blob, on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::TxTypeConflict { .. }), "{err:?}");
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
        let tx = MockTransaction::eip1559().set_sender(tx.sender()).inc_price_by(100).inc_limit();
        let tx = f.validated(tx);
        let err = pool.insert_tx(tx, on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::TxTypeConflict { .. }), "{err:?}");
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
        let unblocked_tx = tx.clone();
        for _ in 0..pool.max_account_slots {
            tx = tx.next();
            pool.insert_tx(f.validated(tx.clone()), on_chain_balance, on_chain_nonce).unwrap();
        }

        assert_eq!(
            pool.max_account_slots,
            pool.tx_count(f.ids.sender_id(tx.get_sender()).unwrap())
        );

        let err =
            pool.insert_tx(f.validated(tx.next()), on_chain_balance, on_chain_nonce).unwrap_err();
        assert!(matches!(err, InsertErr::ExceededSenderTransactionsCapacity { .. }));

        assert!(pool
            .insert_tx(f.validated(unblocked_tx), on_chain_balance, on_chain_nonce)
            .is_ok());
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
            pool.tx_count(f.ids.sender_id(tx.get_sender()).unwrap())
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
}
