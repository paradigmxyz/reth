use crate::{
    error::PoolResult, identifier::TransactionId, traits::PoolTransaction,
    validate::ValidPoolTransaction, TransactionOrdering,
};
use reth_primitives::TxHash;
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
    required_dependencies: HashMap<TransactionId, HashSet<TxHash>>,
    /// Mapping of the dependencies of a transaction to the hash of the transaction,
    waiting_dependencies: HashMap<TransactionId, TxHash>,
    /// Transactions that are not ready yet are waiting for another tx to finish,
    waiting_queue: HashMap<TxHash, QueuedPoolTransaction<T>>,
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
            .get(&tx.transaction.transaction_id)
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
                .entry(*dependency)
                .or_default()
                .insert(*tx.transaction.hash());
        }

        // also track identifying dependencies
        self.waiting_dependencies.insert(tx.transaction.transaction_id, *tx.transaction.hash());

        // add tx to the queue
        self.waiting_queue.insert(*tx.transaction.hash(), tx);

        Ok(())
    }

    /// Returns true if given transaction is part of the queue
    pub(crate) fn contains(&self, hash: &TxHash) -> bool {
        self.waiting_queue.contains_key(hash)
    }

    /// Returns the transaction for the hash if it's waiting
    pub(crate) fn get(&self, tx_hash: &TxHash) -> Option<&QueuedPoolTransaction<T>> {
        self.waiting_queue.get(tx_hash)
    }

    /// Returns the transactions for the given hashes, `None` if no transaction exists
    pub(crate) fn get_all(
        &self,
        tx_hashes: &[TxHash],
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
        id: &TransactionId,
    ) -> Vec<QueuedPoolTransaction<T>> {
        let mut unlocked_ready = Vec::new();
        if let Some(tx_hashes) = self.required_dependencies.remove(id) {
            for hash in tx_hashes {
                let tx = self.waiting_queue.get_mut(&hash).expect("tx is included;");
                tx.satisfy(id);
                if tx.is_satisfied() {
                    let tx = self.waiting_queue.remove(&hash).expect("tx is included;");
                    self.waiting_dependencies.remove(&tx.transaction.transaction_id);
                    unlocked_ready.push(tx);
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
        hashes: Vec<TxHash>,
    ) -> Vec<Arc<ValidPoolTransaction<T::Transaction>>> {
        let mut removed = vec![];
        for hash in hashes {
            if let Some(waiting_tx) = self.waiting_queue.remove(&hash) {
                self.waiting_dependencies.remove(&waiting_tx.transaction.transaction_id);
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
    pub(crate) missing_dependencies: HashSet<TransactionId>,
    /// Timestamp when the tx was added.
    pub(crate) added_at: Instant,
}

impl<T: TransactionOrdering> Default for QueuedTransactions<T> {
    fn default() -> Self {
        Self {
            required_dependencies: Default::default(),
            waiting_dependencies: Default::default(),
            waiting_queue: Default::default(),
        }
    }
}

// === impl QuQueuedPoolTransaction ===

impl<T: TransactionOrdering> QueuedPoolTransaction<T> {
    /// Creates a new `QueuedPoolTransaction`.
    ///
    /// Determines the dependent transaction that are still missing before this transaction can be
    /// moved to the queue.
    pub(crate) fn new(
        transaction: ValidPoolTransaction<T::Transaction>,
        provided: &HashMap<TransactionId, TxHash>,
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
    pub(crate) fn satisfy(&mut self, id: &TransactionId) {
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
