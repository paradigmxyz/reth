use crate::traits::PoolTransaction;
use std::{cmp::Ordering, fmt::Debug, marker::PhantomData};

/// Priority of the transaction that can be missing.
///
/// Transactions with missing priorities are ranked lower.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum Priority<T: Ord + Clone> {
    /// The value of the priority of the transaction.
    Value(T),
    /// Missing priority due to ordering internals.
    None,
}

impl<T: Ord + Clone> From<Option<T>> for Priority<T> {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::None, Priority::Value)
    }
}

impl<T: Ord + Clone> PartialOrd for Priority<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T: Ord + Clone> Ord for Priority<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Value(a), Self::Value(b)) => a.cmp(b),
            // Note: None should be smaller than Value.
            (Self::Value(_), Self::None) => Ordering::Greater,
            (Self::None, Self::Value(_)) => Ordering::Less,
            (Self::None, Self::None) => Ordering::Equal,
        }
    }
}

/// Transaction ordering trait to determine the order of transactions.
///
/// Decides how transactions should be ordered within the pool, depending on a `Priority` value.
///
/// The returned priority must reflect [total order](https://en.wikipedia.org/wiki/Total_order).
pub trait TransactionOrdering: Debug + Send + Sync + 'static {
    /// Priority of a transaction.
    ///
    /// Higher is better.
    type PriorityValue: Ord + Clone + Default + Debug + Send + Sync;

    /// The transaction type to determine the priority of.
    type Transaction: PoolTransaction;

    /// Returns the priority score for the given transaction.
    fn priority(
        &self,
        transaction: &Self::Transaction,
        base_fee: u64,
    ) -> Priority<Self::PriorityValue>;
}

/// Default ordering for the pool.
///
/// The transactions are ordered by their coinbase tip.
/// The higher the coinbase tip is, the higher the priority of the transaction.
#[derive(Debug)]
#[non_exhaustive]
pub struct CoinbaseTipOrdering<T>(PhantomData<T>);

impl<T> TransactionOrdering for CoinbaseTipOrdering<T>
where
    T: PoolTransaction + 'static,
{
    type PriorityValue = u128;
    type Transaction = T;

    /// Source: <https://github.com/ethereum/go-ethereum/blob/7f756dc1185d7f1eeeacb1d12341606b7135f9ea/core/txpool/legacypool/list.go#L469-L482>.
    ///
    /// NOTE: The implementation is incomplete for missing base fee.
    fn priority(
        &self,
        transaction: &Self::Transaction,
        base_fee: u64,
    ) -> Priority<Self::PriorityValue> {
        transaction.effective_tip_per_gas(base_fee).into()
    }
}

impl<T> Default for CoinbaseTipOrdering<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<T> Clone for CoinbaseTipOrdering<T> {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// FIFO ordering for the pool.
///
/// All transactions have equal priority and are processed in the order they were received.
/// Since all transactions return the same priority value, ordering falls back to `submission_id`.
#[derive(Debug)]
#[non_exhaustive]
pub struct FifoOrdering<T>(PhantomData<T>);

impl<T> TransactionOrdering for FifoOrdering<T>
where
    T: PoolTransaction + 'static,
{
    type PriorityValue = u128;
    type Transaction = T;

    fn priority(
        &self,
        _transaction: &Self::Transaction,
        _base_fee: u64,
    ) -> Priority<Self::PriorityValue> {
        Priority::Value(0)
    }
}

impl<T> Default for FifoOrdering<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<T> Clone for FifoOrdering<T> {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// Configurable transaction ordering that can switch between gas-based and FIFO ordering.
///
/// Allows selection between different ordering strategies at pool initialization:
/// - [`CoinbaseTipOrdering`]: Gas price-based ordering (default)
/// - [`FifoOrdering`]: First-in-first-out ordering
///
/// **Note**: The ordering strategy is fixed at pool creation time and cannot be changed
/// during runtime. To switch ordering modes, the pool must be recreated.
#[derive(Debug, Clone)]
pub enum ConfigurableOrdering<T> {
    /// Gas price-based ordering where higher gas tips have higher priority.
    CoinbaseTip(CoinbaseTipOrdering<T>),
    /// FIFO ordering where all transactions have equal priority.
    Fifo(FifoOrdering<T>),
}

impl<T> ConfigurableOrdering<T> {
    /// Creates a new configurable ordering based on the provided flag.
    ///
    /// Returns [`FifoOrdering`] if `fifo_enabled` is true, otherwise [`CoinbaseTipOrdering`].
    pub fn new(fifo_enabled: bool) -> Self {
        if fifo_enabled {
            Self::Fifo(FifoOrdering::default())
        } else {
            Self::CoinbaseTip(CoinbaseTipOrdering::default())
        }
    }
}

impl<T> TransactionOrdering for ConfigurableOrdering<T>
where
    T: PoolTransaction + 'static,
{
    type PriorityValue = u128;
    type Transaction = T;

    fn priority(
        &self,
        transaction: &Self::Transaction,
        base_fee: u64,
    ) -> Priority<Self::PriorityValue> {
        match self {
            Self::CoinbaseTip(ordering) => ordering.priority(transaction, base_fee),
            Self::Fifo(ordering) => ordering.priority(transaction, base_fee),
        }
    }
}

impl<T> Default for ConfigurableOrdering<T> {
    fn default() -> Self {
        Self::CoinbaseTip(CoinbaseTipOrdering::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_ordering() {
        let p1 = Priority::Value(3);
        let p2 = Priority::Value(1);
        let p3 = Priority::None;

        assert!(p1 > p2); // 3 > 1
        assert!(p1 > p3); // Value(3) > None
        assert!(p2 > p3); // Value(1) > None
        assert_eq!(p3, Priority::None);
    }

    #[cfg(any(test, feature = "test-utils"))]
    mod fifo_tests {
        use super::*;
        use crate::test_utils::MockTransaction;

        #[test]
        fn fifo_ordering_returns_constant_priority() {
            let ordering = FifoOrdering::<MockTransaction>::default();
            let tx1 = MockTransaction::legacy();
            let tx2 = MockTransaction::legacy();

            assert_eq!(ordering.priority(&tx1, 1000), Priority::Value(0));
            assert_eq!(ordering.priority(&tx2, 1000), Priority::Value(0));
            assert_eq!(ordering.priority(&tx1, 5000), Priority::Value(0));
        }

        #[test]
        fn configurable_ordering_selects_gas_based() {
            let ordering = ConfigurableOrdering::<MockTransaction>::new(false);
            assert!(matches!(ordering, ConfigurableOrdering::CoinbaseTip(_)));
        }

        #[test]
        fn configurable_ordering_selects_fifo() {
            let ordering = ConfigurableOrdering::<MockTransaction>::new(true);
            assert!(matches!(ordering, ConfigurableOrdering::Fifo(_)));
        }

        #[test]
        fn configurable_ordering_defaults_to_gas_based() {
            let ordering = ConfigurableOrdering::<MockTransaction>::default();
            assert!(matches!(ordering, ConfigurableOrdering::CoinbaseTip(_)));
        }
    }
}
