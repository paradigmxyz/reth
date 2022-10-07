use crate::{
    identifier::{SenderId, TransactionId},
    pool::{pending::PendingTransactions, queued::QueuedTransactions, state::TxState, SenderInfo},
    PoolTransaction, TransactionOrdering, ValidPoolTransaction, U256,
};
use fnv::FnvHashMap;
use reth_primitives::{rpc::transaction, TxHash};
use std::{
    collections::{btree_map::Entry, hash_map, BTreeMap},
    fmt,
    ops::Bound::{Excluded, Included, Unbounded},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
};

/// A pool that only manages transactions.
///
/// This pool maintains a dependency graph of transactions and provides the currently ready
/// transactions.
pub struct TxPool<T: TransactionOrdering> {
    /// How to order transactions.
    ordering: Arc<T>,
    /// Contains the currently known info
    sender_info: FnvHashMap<SenderId, SenderInfo>,
    /// All transactions in the pool.
    all_transactions: AllTransactions<T::Transaction>,
}

/// Container for _all_ transaction in the pool
pub struct AllTransactions<T: PoolTransaction> {
    /// _All_ transaction in the pool sorted by their sender and nonce pair.
    txs: BTreeMap<TransactionId, PoolInternalTransaction<T>>,
    /// Tracks the number of transactions by sender that are currently in the pool.
    tx_counter: FnvHashMap<SenderId, usize>,
}

impl<T: PoolTransaction> AllTransactions<T> {
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

    /// Returns an iterator over all transactions for the given sender, starting with the lowest
    /// nonce
    pub(crate) fn txs_iter<'a>(
        &'a self,
        sender: SenderId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + 'a {
        self.txs
            .range((sender.start_bound(), Unbounded))
            .take_while(move |(other, _)| sender == other.sender)
    }

    /// Returns a mutable iterator over all transactions for the given sender, starting with the
    /// lowest nonce
    pub(crate) fn txs_iter_mut<'a>(
        &'a mut self,
        sender: SenderId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a mut PoolInternalTransaction<T>)> + 'a {
        self.txs
            .range_mut((sender.start_bound(), Unbounded))
            .take_while(move |(other, _)| sender == other.sender)
    }

    /// Returns all transactions that predates the given transaction.
    ///
    /// NOTE: The range is _inclusive_
    pub(crate) fn ancestor_txs<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + 'a {
        self.txs
            .range((Unbounded, Included(id)))
            .rev()
            .take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all mutable transactions that predates the given transaction.
    ///
    /// NOTE: The range is _inclusive_
    pub(crate) fn ancestor_txs_mut<'a, 'b: 'a>(
        &'a mut self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a mut PoolInternalTransaction<T>)> + 'a {
        self.txs
            .range_mut((Unbounded, Included(id)))
            .rev()
            .take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all transactions that predates the given transaction.
    ///
    /// NOTE: The range is _exclusive_: This does not return the transaction itself
    pub(crate) fn ancestor_txs_exclusive<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + 'a {
        self.txs.range(..id).rev().take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id but have the same sender.
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
    /// NOTE: The range is _exclusive_
    pub(crate) fn descendant_txs_exclusive_mut<'a, 'b: 'a>(
        &'a mut self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a mut PoolInternalTransaction<T>)> + 'a {
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

    /// Inserts a new transaction into the pool.
    ///
    /// Returns info to which sub-pool the transaction should be moved.
    pub(crate) fn insert_tx(
        &mut self,
        transaction: Arc<ValidPoolTransaction<T>>,
        on_chain_balance: U256,
        on_chain_nonce: u64,
    ) -> InsertionResult<T> {
        let tx_id = *transaction.id();

        let mut cumulative_cost = U256::zero();

        let predecessor = TransactionId::predecessor(
            transaction.transaction.nonce(),
            on_chain_nonce,
            tx_id.sender,
        );

        if predecessor.is_none() {
            transaction.state.set_bit(TxState::NO_NONCE_GAPS);
        }

        let mut replaced_tx_hash = None;

        // traverse all ancestor transactions
        {
            let mut ancestors = self.ancestor_txs_mut(&tx_id).peekable();
            // If the first existing tx has the same id, then this is a replacement
            if let Some((ancestor_id, ancestor_tx)) = ancestors.peek() {
                if tx_id.eq(*ancestor_id) {
                    // found replacement transaction
                    // TODO check if underpriced

                    // TODO can insert here directly by replacing

                    replaced_tx_hash = Some(*ancestor_tx.transaction.hash());
                    ancestors.next();
                }
            }

            // If the next existing tx is the direct predecessor, then the transaction doesn't have
            // any nonce gaps.
            if let Some((ancestor_id, ancestor_tx)) = ancestors.next() {
                if Some(ancestor_id) == predecessor.as_ref() {
                    transaction.state.set_bit(TxState::NO_NONCE_GAPS);
                    // track cost up to this point
                    cumulative_cost += ancestor_tx.cumulative_cost + ancestor_tx.transaction.cost;
                }
            }
        }

        let is_replacement = replaced_tx_hash.is_some();
        let mut updates = Vec::new();

        // travers in opposite direction to update descendants if there's no nonce gap
        if predecessor.is_none() {
            let mut next_nonce = tx_id.next_nonce();
            let mut next_cumulative_cost = cumulative_cost + transaction.cost;

            for (descendant_id, descendant_tx) in self.descendant_txs_exclusive_mut(&tx_id) {
                if descendant_id.nonce == next_nonce && !is_replacement {
                    // update the nonce gap status
                    descendant_tx.transaction.state.set_bit(TxState::NO_NONCE_GAPS);

                    // TODO compare against allowance
                    descendant_tx.cumulative_cost = next_cumulative_cost;

                    // TODO record state change
                    updates.push(*descendant_id);
                } else {
                    break
                }
                // update cumulative gas used
                next_nonce = descendant_id.next_nonce();
                next_cumulative_cost = descendant_tx.next_cumulative_cost();
            }
        }

        todo!()
    }

    /// Rechecks the transaction of the given sender
    pub(crate) fn on_mined(&mut self, sender: &SenderId, new_balance: U256, old_balance: U256) {
        todo!()
    }
}

/// Where to move an existing transaction
#[derive(Debug)]
pub(crate) enum Destination {
    /// Discard the transaction
    Discard,
    /// Move transaction to pool
    Pool(SubPool),
}

/// A change of the transaction's location
pub(crate) struct MoveTransaction {
    pub(crate) id: TransactionId,
    pub(crate) hash: TxHash,
    /// Where the transaction is currently held.
    pub(crate) current: SubPool,
    /// Where to move the transaction to
    pub(crate) destination: Destination,
}

/// The outcome of [TxPool::insert_tx]
pub(crate) enum InsertionResult<T: PoolTransaction> {
    /// Transaction was successfully inserted into the pool
    Inserted {
        transaction: Arc<ValidPoolTransaction<T>>,
        move_to: SubPool,
        state: TxState,
        replaced: Option<(TxHash, SubPool)>,
        /// Additional updates to transactions affected by this change.
        updates: Vec<MoveTransaction>,
    },
    /// Attempted to replace existing transaction, but was underpriced
    Underpriced { transaction: Arc<ValidPoolTransaction<T>>, existing: TxHash },
}

/// The internal transaction typed used by `AllTransactions` which also additional info used for
/// determining the current state of the transaction.
pub(crate) struct PoolInternalTransaction<T: PoolTransaction> {
    /// The actual transaction object.
    transaction: Arc<ValidPoolTransaction<T>>,
    /// The total cost all transactions before this transaction.
    ///
    /// This is the combined `cost` of all transactions from the same sender that currently
    /// come before this transaction.
    cumulative_cost: U256,
}

// === impl PoolInternalTransaction ===

impl<T: PoolTransaction> PoolInternalTransaction<T> {
    fn new(transaction: Arc<ValidPoolTransaction<T>>) -> Self {
        Self { transaction, cumulative_cost: Default::default() }
    }

    fn next_cumulative_cost(&self) -> U256 {
        self.cumulative_cost + self.transaction.cost
    }
}

/// Identifier for the used Subpool
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum SubPool {
    Queued = 0,
    Pending,
    Parked,
}

// === impl SubPool ===

impl SubPool {
    /// Converts a u8 into the corresponding variant.
    ///
    /// # Panics
    ///
    /// If `val` does not match any variant
    fn from_u8(val: u8) -> Self {
        match val {
            0 => SubPool::Queued,
            1 => SubPool::Pending,
            2 => SubPool::Parked,
            _ => unreachable!("is shielded; qed"),
        }
    }
}

impl From<TxState> for SubPool {
    fn from(value: TxState) -> Self {
        if value > TxState::BASE_FEE_POOL_BITS {
            return SubPool::Pending
        }
        if value < TxState::BASE_FEE_POOL_BITS {
            return SubPool::Queued
        }
        SubPool::Parked
    }
}

/// Helper type that maintains an atomic bool that maps to `SubPool`
pub(crate) struct AtomicSubPool(AtomicU8);

// === impl AtomicSubPool ===

impl AtomicSubPool {
    /// Sets the pool value.
    #[inline]
    pub(crate) fn set(&self, pool: SubPool) {
        self.0.store(pool as u8, Ordering::Relaxed)
    }

    /// Sets the new value and returns the old.
    #[inline]
    pub fn swap(&self, pool: SubPool) -> SubPool {
        let val = self.0.swap(pool as u8, Ordering::Relaxed);
        SubPool::from_u8(val)
    }

    /// Returns the currently held `SubPool` value.
    #[inline]
    pub fn get(&self) -> SubPool {
        let val = self.0.load(Ordering::Relaxed);
        SubPool::from_u8(val)
    }
}

impl fmt::Debug for AtomicSubPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.get().fmt(f)
    }
}
