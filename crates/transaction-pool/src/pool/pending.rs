use crate::{traits::PoolTransaction, validate::ValidPoolTransaction};
use parking_lot::RwLock;
use reth_primitives::{TxHash, H256};
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
pub struct PendingTransactions<T: PoolTransaction> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions where submitted to the pool.
    id: u64,
    /// Markers that are provided by `PendingTransaction`s
    provided_ids: HashMap<T::Id, T::Hash>,
    /// All Transactions that are currently ready.
    ready_transactions: Arc<RwLock<HashMap<T::Hash, PendingTransaction<T>>>>,
    /// Independent transactions that can be included directly and don't require other transactions
    ///
    /// Sorted by their scoring value.
    independent_transactions: BTreeSet<PoolTransactionRef<T>>,
}

/// A transaction that is ready to be included in a block.
#[derive(Debug, Clone)]
pub struct PendingTransaction<T: PoolTransaction> {
    /// Reference to the actual transaction.
    pub transaction: PoolTransactionRef<T>,
    /// Tracks the transactions that get unlocked by this transaction.
    pub unlocks: Vec<T::Hash>,
    /// Amount of required markers that are inherently provided
    pub requires_offset: usize,
}

/// A reference to a transaction in the pool
#[derive(Debug, Clone)]
pub struct PoolTransactionRef<T: PoolTransaction> {
    /// Actual transaction.
    pub transaction: Arc<ValidPoolTransaction<T>>,
    /// Identifier that tags when transaction was submitted in the pool.
    pub submit_id: u64,
    /// The priority value assigned by the used `Scoring` function.
    pub priority: (),
    // TODO add timestamp
}

impl<T: PoolTransaction> Eq for PoolTransactionRef<T> {}

impl<T: PoolTransaction> PartialEq<Self> for PoolTransactionRef<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: PoolTransaction> PartialOrd<Self> for PoolTransactionRef<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: PoolTransaction> Ord for PoolTransactionRef<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        todo!()
    }
}
