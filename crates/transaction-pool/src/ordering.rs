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
    /// Priority is a 3-tuple `(effective_tip, fee_cap, tip_cap)` to enable tie-breaking
    /// when multiple transactions have the same effective tip.
    ///
    /// This matches Geth's transaction ordering behavior.
    type PriorityValue = (u128, u128, u128);
    type Transaction = T;

    /// Returns the priority score for the given transaction.
    ///
    /// Follows the same 3-level comparison as Geth's [`priceHeap.cmp`]:
    /// 1. Primary: effective tip per gas (miner's actual reward)
    /// 2. Secondary: max fee per gas (tie-breaker)
    /// 3. Tertiary: max priority fee per gas (final tie-breaker)
    ///
    /// The tuple automatically provides lexicographic ordering, implementing
    /// the exact same logic as Geth.
    ///
    /// [`priceHeap.cmp`]: https://github.com/ethereum/go-ethereum/blob/7f756dc1185d7f1eeeacb1d12341606b7135f9ea/core/txpool/legacypool/list.go#L469-L482
    fn priority(
        &self,
        transaction: &Self::Transaction,
        base_fee: u64,
    ) -> Priority<Self::PriorityValue> {
        let effective_tip = transaction.effective_tip_per_gas(base_fee).unwrap_or(0);

        let fee_cap = transaction.max_fee_per_gas();

        let tip_cap = transaction.max_priority_fee_per_gas().unwrap_or(0);

        Priority::Value((effective_tip, fee_cap, tip_cap))
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
}
