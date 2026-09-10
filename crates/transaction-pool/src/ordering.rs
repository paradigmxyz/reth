use crate::traits::PoolTransaction;
use alloy_primitives::U256;
use std::{cmp::Ordering, fmt::Debug, marker::PhantomData};

/// Priority of the transaction that can be missing.
///
/// Transactions with missing priorities are ranked lower.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum Priority<T: Ord + Clone> {
    /// The value of the priority of the transaction.
    Value(T),
    /// A fee priority above `u128::MAX`, ranked above every ordinary value.
    ///
    /// Used by coinbase tip ordering without widening custom ordering priority types.
    /// Boxed so ordinary `u128` priorities retain their size; only overflowing tips allocate.
    Overflow(Box<U256>),
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
            (Self::Overflow(a), Self::Overflow(b)) => a.cmp(b),
            (Self::Overflow(_), _) | (Self::Value(_), Self::None) => Ordering::Greater,
            (_, Self::Overflow(_)) | (Self::None, Self::Value(_)) => Ordering::Less,
            (Self::Value(a), Self::Value(b)) => a.cmp(b),
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
        if transaction.frame_transaction().is_some() {
            return match transaction.effective_tip_per_gas_u256(base_fee) {
                Some(tip) => match u128::try_from(tip) {
                    Ok(tip) => Priority::Value(tip),
                    Err(_) => Priority::Overflow(Box::new(tip)),
                },
                None => Priority::None,
            }
        }
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

/// Full-width replacement check when either transaction is a frame transaction.
pub(crate) fn frame_replacement_underpriced<T: PoolTransaction>(
    existing: &T,
    replacement: &T,
    price_bump: u128,
) -> bool {
    if !satisfies_fee_bump(
        existing.max_fee_per_gas_u256(),
        replacement.max_fee_per_gas_u256(),
        price_bump,
    ) {
        return true
    }
    let existing_tip = existing.max_priority_fee_per_gas_u256().unwrap_or_default();
    let replacement_tip = replacement.max_priority_fee_per_gas_u256().unwrap_or_default();
    if !satisfies_fee_bump(existing_tip, replacement_tip, price_bump) {
        return true
    }
    existing.max_fee_per_blob_gas_u256().is_some_and(|fee| {
        !satisfies_fee_bump(
            fee,
            replacement.max_fee_per_blob_gas_u256().unwrap_or_default(),
            price_bump,
        )
    })
}

/// Compute the rounded-up percentage without overflowing the intermediate product.
/// An unrepresentable threshold cannot be met, even by `U256::MAX`.
fn satisfies_fee_bump(existing: U256, replacement: U256, price_bump: u128) -> bool {
    let hundred = U256::from(100);
    let factor = U256::from(price_bump) + hundred;
    let remainder = (existing % hundred) * factor;
    let rounded_remainder =
        remainder / hundred + U256::from((remainder % hundred != U256::ZERO) as u8);
    (existing / hundred)
        .checked_mul(factor)
        .and_then(|whole| whole.checked_add(rounded_remainder))
        .is_some_and(|required| replacement >= required)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EthPooledTransaction, PriceBumpConfig, ValidPoolTransaction};
    use alloy_consensus::TxEip8141;
    use alloy_eips::eip8141::TransactionFees;
    use alloy_primitives::{Address, Sealable};

    fn frame(fee: U256, tip: U256) -> EthPooledTransaction {
        frame_with_blob_fee(fee, tip, None)
    }

    fn frame_with_blob_fee(fee: U256, tip: U256, blob_fee: Option<U256>) -> EthPooledTransaction {
        let tx = TxEip8141 {
            fees: TransactionFees {
                max_fee_per_gas: fee,
                max_priority_fee_per_gas: tip,
                max_fee_per_blob_gas: blob_fee.unwrap_or_default(),
            },
            blob_versioned_hashes: blob_fee
                .map(|_| vec![alloy_primitives::B256::repeat_byte(1)])
                .unwrap_or_default(),
            ..Default::default()
        };
        EthPooledTransaction::new(
            alloy_consensus::transaction::Recovered::new_unchecked(
                reth_ethereum_primitives::TransactionSigned::Eip8141(tx.seal_slow()),
                Address::ZERO,
            ),
            0,
        )
    }

    fn valid(transaction: EthPooledTransaction) -> ValidPoolTransaction<EthPooledTransaction> {
        ValidPoolTransaction {
            transaction,
            transaction_id: crate::identifier::TransactionId::new(
                crate::identifier::SenderId::from(0),
                0,
            ),
            propagate: true,
            timestamp: std::time::Instant::now(),
            origin: crate::TransactionOrigin::External,
            authority_ids: None,
        }
    }

    #[test]
    fn frame_subpool_ordering_retains_high_bits() {
        use crate::pool::{pending::PendingTransaction, BasefeeOrd, QueuedOrd};
        use std::sync::Arc;

        let fee = U256::from(u128::MAX) + U256::from(1);
        let low = Arc::new(valid(frame(fee, fee)));
        let high = Arc::new(valid(frame(fee + U256::from(1), fee + U256::from(1))));
        assert!(BasefeeOrd::from(high.clone()) > BasefeeOrd::from(low.clone()));
        assert!(QueuedOrd::from(high.clone()) > QueuedOrd::from(low.clone()));
        let ordering = CoinbaseTipOrdering::default();
        let low: PendingTransaction<CoinbaseTipOrdering<EthPooledTransaction>> =
            PendingTransaction {
                submission_id: 0,
                priority: ordering.priority(&low.transaction, 0),
                transaction: low,
            };
        let high: PendingTransaction<CoinbaseTipOrdering<EthPooledTransaction>> =
            PendingTransaction {
                submission_id: 1,
                priority: ordering.priority(&high.transaction, 0),
                transaction: high,
            };
        // The higher fee wins even though it was submitted later.
        assert!(high > low);
    }

    #[test]
    fn frame_priority_retains_high_bits_and_subtracts_basefee() {
        let ordering = CoinbaseTipOrdering::default();
        let boundary = U256::from(u128::MAX);
        let low = frame(boundary + U256::from(1), U256::MAX);
        let high = frame(boundary + U256::from(2), U256::MAX);
        assert_eq!(alloy_consensus::Transaction::priority_fee_or_price_u256(&high), U256::MAX);
        assert!(ordering.priority(&high, 0) > ordering.priority(&low, 0));
        assert!(ordering.priority(&low, 0) > Priority::Value(u128::MAX));
        assert_eq!(ordering.priority(&low, 1), Priority::Value(u128::MAX));
        assert_eq!(ordering.priority(&low, 2), Priority::Value(u128::MAX - 1));
        assert_eq!(ordering.priority(&frame(boundary, U256::from(7)), 1), Priority::Value(7));
        assert_eq!(ordering.priority(&frame(U256::from(1), U256::from(1)), 2), Priority::None);
    }

    #[test]
    fn frame_bump_rounds_up_and_rejects_overflow() {
        let fee = U256::from(u128::MAX) + U256::from(1);
        let required = (fee * U256::from(110) + U256::from(99)) / U256::from(100);
        assert!(!satisfies_fee_bump(fee, fee, 10));
        assert!(!satisfies_fee_bump(fee, required - U256::from(1), 10));
        assert!(satisfies_fee_bump(fee, required, 10));
        assert!(!satisfies_fee_bump(U256::MAX, U256::MAX, 10));
        assert!(satisfies_fee_bump(U256::MAX, U256::MAX, 0));
        assert!(!satisfies_fee_bump(U256::MAX, U256::MAX, u128::MAX));
        assert!(satisfies_fee_bump(U256::from(1), U256::from(u128::MAX), u128::MAX));
        assert!(!satisfies_fee_bump(U256::from(1), U256::from(1), 10));
        assert!(satisfies_fee_bump(U256::from(1), U256::from(2), 10));
    }

    #[test]
    fn frame_replacement_checks_full_fee_and_tip() {
        let fee = (U256::from(u128::MAX) + U256::from(1)) * U256::from(100);
        let bumped = fee / U256::from(100) * U256::from(110);
        let existing = valid(frame(fee, fee));
        let config = PriceBumpConfig::default();
        assert!(existing.is_underpriced(&valid(frame(fee, fee)), &config));
        assert!(existing.is_underpriced(&valid(frame(bumped, fee)), &config));
        assert!(existing.is_underpriced(&valid(frame(bumped, U256::ZERO)), &config));
        assert!(existing.is_underpriced(&valid(frame(bumped - U256::from(1), bumped)), &config));
        assert!(!existing.is_underpriced(&valid(frame(bumped, bumped)), &config));
        let maximum = valid(frame(U256::MAX, U256::MAX));
        assert!(maximum.is_underpriced(&maximum, &config));
    }

    #[test]
    fn frame_blob_replacement_uses_blob_price_bump() {
        let fee = U256::from(u128::MAX) + U256::from(1);
        let config = PriceBumpConfig { default_price_bump: 10, replace_blob_tx_price_bump: 100 };
        let existing = valid(frame_with_blob_fee(fee, fee, Some(fee)));
        let doubled = fee * U256::from(2);
        let below = doubled - U256::from(1);
        assert!(existing
            .is_underpriced(&valid(frame_with_blob_fee(doubled, doubled, Some(below))), &config));
        assert!(existing
            .is_underpriced(&valid(frame_with_blob_fee(below, doubled, Some(doubled))), &config));
        assert!(existing
            .is_underpriced(&valid(frame_with_blob_fee(doubled, below, Some(doubled))), &config));
        assert!(!existing
            .is_underpriced(&valid(frame_with_blob_fee(doubled, doubled, Some(doubled))), &config));
    }

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
