use crate::{
    identifier::{SenderId, TransactionId},
    pool::size::SizeTracker,
    PoolTransaction, ValidPoolTransaction,
};
use fnv::FnvHashMap;
use std::{cmp::Ordering, collections::BTreeSet, ops::Deref, sync::Arc};

/// A pool of transactions that are currently parked and are waiting for external changes (e.g.
/// basefee, ancestor transactions, balance) that eventually move the transaction into the pending
/// pool.
///
/// This pool is a bijection: at all times each set (`best`, `by_id`) contains the same
/// transactions.
///
/// Note: This type is generic over [ParkedPool] which enforces that the underlying transaction type
/// is [ValidPoolTransaction] wrapped in an [Arc].
pub(crate) struct ParkedPool<T: ParkedOrd> {
    /// Keeps track of transactions inserted in the pool.
    ///
    /// This way we can determine when transactions where submitted to the pool.
    submission_id: u64,
    /// All transactions grouped by their senders.
    all: FnvHashMap<SenderId, BTreeSet<ParkedPoolTransaction<T>>>,
    /// The collection of single transaction per sender ordered by their
    /// order function.
    best: BTreeSet<ParkedPoolTransaction<T>>,
    /// Keeps track of the size of this pool.
    ///
    /// See also [`PoolTransaction::size`].
    size_of: SizeTracker,
    /// Keeps track of the pool len.
    len: usize,
}

// === impl ParkedPool ===

impl<T: ParkedOrd> ParkedPool<T> {
    /// Adds a new transactions to the pending queue.
    ///
    /// # Panics
    ///
    /// If the transaction is already included.
    pub(crate) fn add_transaction(&mut self, tx: Arc<ValidPoolTransaction<T::Transaction>>) {
        let id = *tx.id();
        assert!(!self.exists_by_id(&id), "transaction already included");
        let submission_id = self.next_id();

        // keep track of size
        self.size_of += tx.size();
        // keep track of len
        self.len += 1;

        let transaction: ParkedPoolTransaction<T> =
            ParkedPoolTransaction { submission_id, transaction: tx.into() };

        if let Some(existing) =
            self.best.iter().find(|t| t.transaction.sender_id() == id.sender).cloned()
        {
            if transaction.transaction.nonce() < existing.transaction.nonce() {
                self.best.remove(&existing);
                self.best.insert(transaction.clone());
            }
        }

        self.all.entry(id.sender).or_default().insert(transaction);
    }

    /// Removes the transaction from the pool
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        // Find target transaction
        let sender_txs = self.all.get_mut(&id.sender)?;
        let tx = sender_txs.iter().find(|tx| tx.transaction.id().eq(id)).cloned()?;

        // Remove it from all transactions collection
        sender_txs.remove(&tx);
        // Remove it from best transactions if it's there
        self.best.remove(&tx);

        // keep track of size
        self.size_of -= tx.transaction.size();
        // keep track of len
        self.len -= 1;

        Some(tx.transaction.into())
    }

    /// Removes the worst transaction from this pool.
    pub(crate) fn pop_worst(&mut self) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let worst = self.get_worst()?;
        self.remove_transaction(&worst)
    }

    fn get_worst(&self) -> Option<TransactionId> {
        // Retrieve worst transaction id from ordered list.
        let worst_id = self.best.first().map(|tx| *tx.transaction.id())?;
        // Retrieve the highest nonce transaction for the sender with the worst transaction.
        self.all.get(&worst_id.sender).and_then(|txs| txs.last()).map(|tx| *tx.transaction.id())
    }

    fn exists_by_id(&self, id: &TransactionId) -> bool {
        self.all
            .get(&id.sender)
            .map(|txs| txs.iter().any(|tx| tx.transaction.id().eq(id)))
            .unwrap_or_default()
    }

    fn next_id(&mut self) -> u64 {
        let id = self.submission_id;
        self.submission_id = self.submission_id.wrapping_add(1);
        id
    }

    /// The reported size of all transactions in this pool.
    pub(crate) fn size(&self) -> usize {
        self.size_of.into()
    }

    /// Number of transactions in the entire pool
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Whether the pool is empty
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl<T: ParkedOrd> Default for ParkedPool<T> {
    fn default() -> Self {
        Self {
            submission_id: 0,
            all: Default::default(),
            best: Default::default(),
            size_of: Default::default(),
            len: 0,
        }
    }
}

/// Represents a transaction in this pool.
struct ParkedPoolTransaction<T: ParkedOrd> {
    /// Identifier that tags when transaction was submitted in the pool.
    submission_id: u64,
    /// Actual transaction.
    transaction: T,
}

impl<T: ParkedOrd> Clone for ParkedPoolTransaction<T> {
    fn clone(&self) -> Self {
        Self { submission_id: self.submission_id, transaction: self.transaction.clone() }
    }
}

impl<T: ParkedOrd> Eq for ParkedPoolTransaction<T> {}

impl<T: ParkedOrd> PartialEq<Self> for ParkedPoolTransaction<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T: ParkedOrd> PartialOrd<Self> for ParkedPoolTransaction<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: ParkedOrd> Ord for ParkedPoolTransaction<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // The first order comparison function depends on the context of the comparison.
        // The collection guarantees that the contexts do not overlap.
        //
        // 1. If the senders are the same, we assume that all of the transactions we are
        //    comparing are from the same sender. The nonce will be used for determining the order.
        //
        // 2. If the senders are different, we assume that all of the transactions we are
        //    comparing are from different senders. The underlying order function will be used for
        //    determining the order.
        let first_order_cmp = if self.transaction.sender_id().eq(&other.transaction.sender_id()) {
            self.transaction.nonce().cmp(&other.transaction.nonce())
        } else {
            self.transaction.cmp(&other.transaction)
        };

        // If the transactions are equal according to the first order comparison function,
        // then we will use the unique `submission_id`. Better transactions have greater
        // `submission_id`.
        first_order_cmp.then_with(|| other.submission_id.cmp(&self.submission_id))
    }
}

/// Helper trait used for custom `Ord` wrappers around a transaction.
///
/// This is effectively a wrapper for `Arc<ValidPoolTransaction>` with custom `Ord` implementation.
pub(crate) trait ParkedOrd:
    Ord
    + Clone
    + From<Arc<ValidPoolTransaction<Self::Transaction>>>
    + Into<Arc<ValidPoolTransaction<Self::Transaction>>>
    + Deref<Target = Arc<ValidPoolTransaction<Self::Transaction>>>
{
    /// The wrapper transaction type.
    type Transaction: PoolTransaction;
}

/// Helper macro to implement necessary conversions for `ParkedOrd` trait
macro_rules! impl_ord_wrapper {
    ($name:ident) => {
        impl<T: PoolTransaction> Clone for $name<T> {
            fn clone(&self) -> Self {
                Self(self.0.clone())
            }
        }

        impl<T: PoolTransaction> Eq for $name<T> {}

        impl<T: PoolTransaction> PartialEq<Self> for $name<T> {
            fn eq(&self, other: &Self) -> bool {
                self.cmp(other) == Ordering::Equal
            }
        }

        impl<T: PoolTransaction> PartialOrd<Self> for $name<T> {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }
        impl<T: PoolTransaction> Deref for $name<T> {
            type Target = Arc<ValidPoolTransaction<T>>;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl<T: PoolTransaction> ParkedOrd for $name<T> {
            type Transaction = T;
        }

        impl<T: PoolTransaction> From<Arc<ValidPoolTransaction<T>>> for $name<T> {
            fn from(value: Arc<ValidPoolTransaction<T>>) -> Self {
                Self(value)
            }
        }

        impl<T: PoolTransaction> From<$name<T>> for Arc<ValidPoolTransaction<T>> {
            fn from(value: $name<T>) -> Arc<ValidPoolTransaction<T>> {
                value.0
            }
        }
    };
}

/// A new type wrapper for [`ValidPoolTransaction`]
///
/// This sorts transactions by their base fee.
///
/// Caution: This assumes all transaction in the `BaseFee` sub-pool have a fee value.
#[derive(Debug)]
pub(crate) struct BasefeeOrd<T: PoolTransaction>(Arc<ValidPoolTransaction<T>>);

impl_ord_wrapper!(BasefeeOrd);

impl<T: PoolTransaction> Ord for BasefeeOrd<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.0.transaction.max_fee_per_gas(), other.0.transaction.max_fee_per_gas()) {
            (Some(fee), Some(other)) => fee.cmp(&other),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            _ => Ordering::Equal,
        }
    }
}

/// A new type wrapper for [`ValidPoolTransaction`]
///
/// This sorts transactions by their distance.
///
/// `Queued` transactions are transactions that are currently blocked by other parked (basefee,
/// queued) or missing transactions.
///
/// The primary order function always compares the transaction costs first. In case these
/// are equal, it compares the timestamps when the transactions were created.
#[derive(Debug)]
pub(crate) struct QueuedOrd<T: PoolTransaction>(Arc<ValidPoolTransaction<T>>);

impl_ord_wrapper!(QueuedOrd);

impl<T: PoolTransaction> Ord for QueuedOrd<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher cost is better
        self.cost.cmp(&other.cost).then_with(||
            // Lower timestamp is better
            other.timestamp.cmp(&self.timestamp))
    }
}
