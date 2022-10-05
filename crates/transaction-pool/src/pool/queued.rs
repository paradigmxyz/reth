use crate::{
    error::PoolResult,
    pool::{TransactionHashFor, TransactionIdFor},
    traits::PoolTransaction,
    validate::ValidPoolTransaction,
    TransactionOrdering,
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
    time::Instant,
};

/// A pool of transactions that are not ready on the current state and are waiting for state changes
/// that turn them valid.
///
/// This could include transactions with nonce gaps: Transactions that are waiting until for a
/// transaction to arrive that closes the nonce gap.
///
/// Keeps a set of transactions that are waiting until their dependencies are unlocked.
pub(crate) struct QueuedTransactions<T: TransactionOrdering> {
    /// Dependencies that aren't yet provided by any transaction.
    required_dependencies: HashMap<TransactionIdFor<T>, HashSet<TransactionHashFor<T>>>,
    /// Mapping of the dependencies of a transaction to the hash of the transaction,
    waiting_dependencies: HashMap<Vec<TransactionIdFor<T>>, TransactionHashFor<T>>,
    /// Transactions that are not ready yet are waiting for another tx to finish,
    waiting_queue: HashMap<TransactionHashFor<T>, QueuedPoolTransaction<T>>,
}

// == impl QueuedTransactions ==

impl<T: TransactionOrdering> QueuedTransactions<T> {
    /// Returns the number of transactions that are currently waiting in this pool for new
    /// transactions to satisfy their dependencies.
    pub(crate) fn len(&self) -> usize {
        self.waiting_queue.len()
    }

    /// Whether this pool is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.waiting_queue.is_empty()
    }

    /// Returns an iterator over all transactions waiting in this pool.
    pub(crate) fn transactions(
        &self,
    ) -> impl Iterator<Item = Arc<ValidPoolTransaction<T::Transaction>>> + '_ {
        self.waiting_queue.values().map(|tx| Arc::clone(&tx.transaction))
    }

    /// Adds a transaction to the queue of transactions
    pub(crate) fn add_transaction(&mut self, tx: QueuedPoolTransaction<T>) -> PoolResult<()> {
        assert!(!tx.is_satisfied(), "transaction must not be ready");
        assert!(
            !self.waiting_queue.contains_key(tx.transaction.hash()),
            "transaction is already added"
        );

        if let Some(_replace) = self
            .waiting_dependencies
            .get(&tx.transaction.provides)
            .and_then(|hash| self.waiting_queue.get(hash))
        {
            // TODO handle transaction underpriced
            // // check if underpriced
            // if tx.transaction.gas_price() < replace.transaction.gas_price() {
            //     warn!(target: "txpool", "pending replacement transaction underpriced [{:?}]",
            // tx.transaction.hash());     return Err(Error::ReplacementUnderpriced)
            // }
        }

        // add all missing dependencies
        for dependency in &tx.missing_dependencies {
            self.required_dependencies
                .entry(dependency.clone())
                .or_default()
                .insert(*tx.transaction.hash());
        }

        // also track identifying dependencies
        self.waiting_dependencies.insert(tx.transaction.provides.clone(), *tx.transaction.hash());

        // add tx to the queue
        self.waiting_queue.insert(*tx.transaction.hash(), tx);

        Ok(())
    }

    /// Returns true if given transaction is part of the queue
    pub(crate) fn contains(&self, hash: &TransactionHashFor<T>) -> bool {
        self.waiting_queue.contains_key(hash)
    }

    /// Returns the transaction for the hash if it's waiting
    pub(crate) fn get(&self, tx_hash: &TransactionHashFor<T>) -> Option<&QueuedPoolTransaction<T>> {
        self.waiting_queue.get(tx_hash)
    }

    /// Returns the transactions for the given hashes, `None` if no transaction exists
    pub(crate) fn get_all(
        &self,
        tx_hashes: &[TransactionHashFor<T>],
    ) -> Vec<Option<Arc<ValidPoolTransaction<T::Transaction>>>> {
        tx_hashes
            .iter()
            .map(|hash| self.waiting_queue.get(hash).map(|tx| Arc::clone(&tx.transaction)))
            .collect()
    }

    /// This will check off the dependencies of queued transactions.
    ///
    /// Returns the those transactions that become unlocked (all dependencies checked) and can be
    /// moved to the ready queue.
    pub(crate) fn satisfy_and_unlock(
        &mut self,
        dependencies: impl IntoIterator<Item = impl AsRef<TransactionIdFor<T>>>,
    ) -> Vec<QueuedPoolTransaction<T>> {
        let mut unlocked_ready = Vec::new();
        for dependency in dependencies {
            let mark = dependency.as_ref();
            if let Some(tx_hashes) = self.required_dependencies.remove(mark) {
                for hash in tx_hashes {
                    let tx = self.waiting_queue.get_mut(&hash).expect("tx is included;");
                    tx.satisfy(mark);

                    if tx.is_satisfied() {
                        let tx = self.waiting_queue.remove(&hash).expect("tx is included;");
                        self.waiting_dependencies.remove(&tx.transaction.provides);

                        unlocked_ready.push(tx);
                    }
                }
            }
        }

        unlocked_ready
    }

    /// Removes the transactions associated with the given hashes
    ///
    /// Returns all removed transactions.
    pub(crate) fn remove(
        &mut self,
        hashes: Vec<TransactionHashFor<T>>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = vec![];
        for hash in hashes {
            if let Some(waiting_tx) = self.waiting_queue.remove(&hash) {
                self.waiting_dependencies.remove(&waiting_tx.transaction.provides);
                for dependency in waiting_tx.missing_dependencies {
                    let remove =
                        if let Some(required) = self.required_dependencies.get_mut(&dependency) {
                            required.remove(&hash);
                            required.is_empty()
                        } else {
                            false
                        };
                    if remove {
                        self.required_dependencies.remove(&dependency);
                    }
                }
                removed.push(waiting_tx.transaction)
            }
        }
        removed
    }
}

/// A transaction submitted to the pool.
#[derive(Clone)]
pub(crate) struct QueuedPoolTransaction<T: TransactionOrdering> {
    /// The actual validated transaction.
    pub(crate) transaction: Arc<ValidPoolTransaction<T::Transaction>>,
    /// Transactions required for and have not been satisfied yet by other transactions in the
    /// pool.
    ///
    /// This will be an empty list if there are no nonce gaps across multiple transactions of the
    /// same sender in the pool. If there are gaps, this will include the missing transactions.
    pub(crate) missing_dependencies: HashSet<TransactionIdFor<T>>,
    /// Timestamp when the tx was added.
    pub(crate) added_at: Instant,
}

// === impl QuQueuedPoolTransaction ===

impl<T: TransactionOrdering> QueuedPoolTransaction<T> {
    /// Creates a new `QueuedPoolTransaction`.
    ///
    /// Determines the dependent transaction that are still missing before this transaction can be
    /// moved to the queue.
    pub(crate) fn new(
        transaction: ValidPoolTransaction<T::Transaction>,
        provided: &HashMap<TransactionIdFor<T>, TransactionHashFor<T>>,
    ) -> Self {
        let missing_dependencies = transaction
            .depends_on
            .iter()
            .filter(|id| {
                // is true if the dependency id is already satisfied either via transaction in the
                // pool
                !provided.contains_key(&**id)
            })
            .cloned()
            .collect();

        Self { transaction: Arc::new(transaction), missing_dependencies, added_at: Instant::now() }
    }

    /// Removes the required dependency.
    pub(crate) fn satisfy(&mut self, id: &TransactionIdFor<T>) {
        self.missing_dependencies.remove(id);
    }

    /// Returns true if transaction has all dependencies are satisfied.
    pub(crate) fn is_satisfied(&self) -> bool {
        self.missing_dependencies.is_empty()
    }
}

impl<T: TransactionOrdering> fmt::Debug for QueuedPoolTransaction<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(fmt, "QueuedPoolTransaction {{ ")?;
        write!(fmt, "added_at: {:?}, ", self.added_at)?;
        write!(fmt, "tx: {:?}, ", self.transaction)?;
        write!(fmt, "missing_dependencies: [{:?}]", &self.missing_dependencies)?;
        write!(fmt, "}}")
    }
}
