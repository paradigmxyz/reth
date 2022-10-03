use crate::{traits::PoolTransaction, validate::ValidPoolTransaction};
use reth_primitives::TxHash;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

/// A pool of transactions that are not ready on the current state and are waiting for state changes
/// that turn them valid.
///
/// This could include:
///     - transactions that are waiting until a pending or queued transactions are mined
///     - state changes that turns them valid (e.g. basefee)
///
/// Keeps a set of transactions that are waiting until their dependencies are unlocked.
pub(crate) struct QueuedTransactions<T: PoolTransaction> {
    /// Dependencies that aren't yet provided by any transaction.
    dependencies: HashMap<T::Id, HashSet<T::Hash>>,
    /// Mapping of the dependencies of a transaction to the hash of the transaction
    waiting: HashMap<Vec<T::Id>, T::Hash>,
    /// the transactions that are not ready yet are waiting for another tx to finish
    waiting_queue: HashMap<T::Hash, QueuedPoolTransaction<T>>,
}

/// A transaction in the pool
#[derive(Clone)]
pub struct QueuedPoolTransaction<T: PoolTransaction> {
    /// Actual transaction.
    pub transaction: Arc<ValidPoolTransaction<T>>,
    /// All Ids required and have not been satisfied yet by other transactions in the pool.
    pub missing_dependencies: HashSet<T::Id>,
    /// Timestamp when the tx was added
    pub added_at: Instant,
}
