use crate::{
    identifier::TransactionId, pool::size::SizeTracker, PoolTransaction, ValidPoolTransaction,
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
    /// _All_ Transactions that are currently inside the pool grouped by their identifier.
    by_id: FnvHashMap<TransactionId, ParkedPoolTransaction<T>>,
    /// All transactions sorted by their order function.
    ///
    /// The higher, the better.
    best: BTreeSet<ParkedPoolTransaction<T>>,
    /// Keeps track of the size of this pool.
    ///
    /// See also [`PoolTransaction::size`].
    size_of: SizeTracker,
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
        assert!(
            !self.by_id.contains_key(&id),
            "transaction already included {:?}",
            self.by_id.contains_key(&id)
        );
        let submission_id = self.next_id();

        // keep track of size
        self.size_of += tx.size();

        let transaction = ParkedPoolTransaction { submission_id, transaction: tx.into() };

        self.by_id.insert(id, transaction.clone());
        self.best.insert(transaction);
    }

    /// Removes the transaction from the pool
    pub(crate) fn remove_transaction(
        &mut self,
        id: &TransactionId,
    ) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        // remove from queues
        let tx = self.by_id.remove(id)?;
        self.best.remove(&tx);

        // keep track of size
        self.size_of -= tx.transaction.size();

        Some(tx.transaction.into())
    }

    /// Removes the worst transaction from this pool.
    pub(crate) fn pop_worst(&mut self) -> Option<Arc<ValidPoolTransaction<T::Transaction>>> {
        let worst = self.best.iter().next().map(|tx| *tx.transaction.id())?;
        self.remove_transaction(&worst)
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
        self.by_id.len()
    }

    /// Whether the pool is empty
    #[cfg(test)]
    #[allow(unused)]
    pub(crate) fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

impl<T: ParkedOrd> Default for ParkedPool<T> {
    fn default() -> Self {
        Self {
            submission_id: 0,
            by_id: Default::default(),
            best: Default::default(),
            size_of: Default::default(),
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
        // This compares by the transactions first, and only if two tx are equal this compares
        // the unique `submission_id`.
        // "better" transactions are Greater
        self.transaction
            .cmp(&other.transaction)
            .then_with(|| other.submission_id.cmp(&self.submission_id))
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
