use crate::{traits::PoolTransaction, validate::ValidPoolTransaction, TransactionOrdering};
use parking_lot::RwLock;
use reth_primitives::{TxHash, H256, U256};
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap},
    sync::Arc,
};

/// A pool of validated transactions that are ready on the current state and are waiting to be
/// included in a block.
///
/// Each transaction in this pool is valid on its own, i.e. they are not dependent on transaction
/// that must be executed first. Each of these transaction can be executed independently on the
/// current state
pub(crate) struct PendingTransactions<T: PoolTransaction, O: TransactionOrdering> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions where submitted to the pool.
    id: u64,
    /// Dependencies that are provided by `PendingTransaction`s
    provided_ids: HashMap<T::Id, T::Hash>,
    /// Pending transactions that are currently on hold until the `baseFee` of the pending block
    /// changes in favor of the parked transactions: the `pendingBlock.baseFee` must decrease
    /// before they can be moved to the ready pool and are ready to be executed.
    parked: ParkedTransactions<T, O>,
    /// All Transactions that are currently ready.
    ///
    /// Meaning, there are no nonce gaps in these transactions and all of them satisfy the
    /// `baseFee` condition: transaction `maxFeePerGas >= pendingBlock.baseFee`
    ready_transactions: Arc<RwLock<HashMap<T::Hash, PendingTransaction<T, O>>>>,
    /// Independent transactions that can be included directly and don't require other
    /// transactions.
    ///
    /// Sorted by their scoring value.
    independent_transactions: BTreeSet<PoolTransactionRef<T, O>>,
}

/// A transaction that is ready to be included in a block.
#[derive(Debug, Clone)]
pub struct PendingTransaction<T: PoolTransaction, O: TransactionOrdering> {
    /// Reference to the actual transaction.
    pub transaction: PoolTransactionRef<T, O>,
    /// Tracks the transactions that get unlocked by this transaction.
    pub unlocks: Vec<T::Hash>,
    /// Amount of required markers that are inherently provided
    pub requires_offset: usize,
}

/// A reference to a transaction in the _pending_ pool
#[derive(Debug, Clone)]
pub struct PoolTransactionRef<T: PoolTransaction, O: TransactionOrdering> {
    /// Actual transaction.
    pub transaction: Arc<ValidPoolTransaction<T>>,
    /// Identifier that tags when transaction was submitted in the pool.
    pub submission_id: u64,
    /// The priority value assigned by the used `Ordering` function.
    pub priority: O::Priority,
}

impl<T: PoolTransaction, O: TransactionOrdering> Eq for PoolTransactionRef<T, O> {}

impl<T: PoolTransaction, O: TransactionOrdering> PartialEq<Self> for PoolTransactionRef<T, O> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: PoolTransaction, O: TransactionOrdering> PartialOrd<Self> for PoolTransactionRef<T, O> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: PoolTransaction, O: TransactionOrdering> Ord for PoolTransactionRef<T, O> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares by `priority` and only if two tx have the exact same priority this compares
        // the unique `submission_id`. This ensures that transactions with same priority are not
        // equal, so they're not replaced in the set
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}

/// Pending Transactions that are currently parked until their set baseFee becomes valid
struct ParkedTransactions<T: PoolTransaction, O: TransactionOrdering> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions where submitted to the pool.
    id: u64,
    /// All transactions that are currently parked due to their fee.
    parked_transactions: HashMap<T::Hash, ParkedTransaction<T, O>>,
    /// Same transactions but sorted by their fee and priority
    sorted_transactions: BTreeSet<ParkedTransactionRef<T, O>>,
}

/// A transaction that is ready to be included in a block.
#[derive(Debug, Clone)]
pub struct ParkedTransaction<T: PoolTransaction, O: TransactionOrdering> {
    /// Reference to the actual transaction.
    transaction: PoolTransactionRef<T, O>,
    /// Tracks the transactions that get unlocked by this transaction.
    unlocks: Vec<T::Hash>,
    /// Amount of required markers that are inherently provided
    requires_offset: usize,
}

/// A reference to a currently _parked_ transaction.
struct ParkedTransactionRef<T: PoolTransaction, O: TransactionOrdering> {
    /// Actual transaction.
    transaction: Arc<ValidPoolTransaction<T>>,
    /// Identifier that tags when transaction was submitted in the pool.
    submission_id: u64,
    /// The priority value assigned by the used `Ordering` function.
    priority: O::Priority,
    /// EIP-1559 Max base fee the caller is willing to pay.
    max_fee_per_gas: U256,
}

impl<T: PoolTransaction, O: TransactionOrdering> Eq for ParkedTransactionRef<T, O> {}

impl<T: PoolTransaction, O: TransactionOrdering> PartialEq<Self> for ParkedTransactionRef<T, O> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: PoolTransaction, O: TransactionOrdering> PartialOrd<Self> for ParkedTransactionRef<T, O> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: PoolTransaction, O: TransactionOrdering> Ord for ParkedTransactionRef<T, O> {
    fn cmp(&self, other: &Self) -> Ordering {
        // This compares the `max_fee_per_gas` value of the transaction
        self.max_fee_per_gas
            .cmp(&other.max_fee_per_gas)
            .then_with(|| self.priority.cmp(&other.priority))
            .then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}
