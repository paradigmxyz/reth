use crate::{
    identifier::{SenderId, TransactionId},
    pool::{pending::PendingTransactions, queued::QueuedTransactions, state::TxState, SenderInfo},
    PoolTransaction, TransactionOrdering, ValidPoolTransaction, U256,
};
use fnv::FnvHashMap;
use reth_primitives::TxHash;
use std::{
    collections::{hash_map::Entry, BTreeMap},
    ops::Bound::{Excluded, Included, Unbounded},
    sync::Arc,
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
        if let Entry::Occupied(mut entry) = self.tx_counter.entry(sender) {
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

    /// Returns a mutable iterator over all transactions for the given sender, starting with the lowest
    /// nonce
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
    /// NOTE: The range is _exclusive_: This does not return the transaction itself
    pub(crate) fn previous_txs_exclusive<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + 'a {
        self.txs.range(..id).rev().take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id but have the same sender.
    ///
    /// NOTE: The range is _exclusive_
    pub(crate) fn subsequent_txs_exclusive<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + 'a {
        self.txs.range((Excluded(id), Unbounded)).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all transactions that _follow_ after the given id but have the same sender.
    ///
    /// NOTE: The range is _inclusive_: if the transaction that belongs to `id` it field be the
    /// first value.
    pub(crate) fn subsequent_txs<'a, 'b: 'a>(
        &'a self,
        id: &'b TransactionId,
    ) -> impl Iterator<Item = (&'a TransactionId, &'a PoolInternalTransaction<T>)> + 'a {
        self.txs.range(id..).take_while(|(other, _)| id.sender == other.sender)
    }

    /// Returns all mutable transactions that _follow_ after the given id but have the same sender.
    ///
    /// NOTE: The range is _inclusive_: if the transaction that belongs to `id` it field be the
    /// first value.
    pub(crate) fn subsequent_txs_mut<'a, 'b: 'a>(
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
        todo!()
    }

    /// Rechecks the transaction of the given sender
    pub(crate) fn on_mined(&mut self, sender: &SenderId, new_balance: U256, old_balance: U256) {
        todo!()
    }
}

/// The outcome of [TxPool::insert_tx]
pub(crate) enum InsertionResult<T: PoolTransaction> {
    /// Transaction was successfully inserted into the pool
    Inserted {
        transaction: Arc<ValidPoolTransaction<T>>,
        move_to: SubPool,
        state: TxState,
        replaced: Option<(TxHash, SubPool)>,
    },
    /// Attempted to replace existing transaction, but was underpriced
    Underpriced { transaction: Arc<ValidPoolTransaction<T>>, existing: TxHash },
}

/// The internal transaction typed used by `AllTransactions` which also additional info used for
/// determining the current state of the transaction.
pub(crate) struct PoolInternalTransaction<T: PoolTransaction> {
    /// Keeps track of the current state of the transaction and therefor in which subpool it should
    /// reside
    state: TxState,
    /// The actual transaction object.
    transaction: Arc<ValidPoolTransaction<T>>,
    /// In which sub pool the transaction currently resides
    sub_pool: SubPool,
    /// The amount of gas needs to be available in the sender's balance.
    ///
    /// This is the combined `gas_used` of all transactions from the same sender that currently
    /// come before this transaction.
    cumulative_gas_used: U256,
}

///
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum SubPool {
    Queued,
    Pending,
    Parked,
}
