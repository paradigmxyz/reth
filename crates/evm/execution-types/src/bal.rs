use core::num::NonZeroU64;

/// Selects blocks that contain enough transactions to benefit from BAL execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BalExecutionPolicy {
    min_transactions: NonZeroU64,
}

impl BalExecutionPolicy {
    /// Creates a policy with the inclusive minimum transaction count.
    pub const fn new(min_transactions: NonZeroU64) -> Self {
        Self { min_transactions }
    }

    /// Returns whether `transaction_count` meets the inclusive minimum.
    pub const fn is_eligible(&self, transaction_count: u64) -> bool {
        transaction_count >= self.min_transactions.get()
    }
}

#[cfg(test)]
mod tests {
    use super::BalExecutionPolicy;
    use core::num::NonZeroU64;

    #[test]
    fn rejects_transaction_count_below_minimum() {
        let policy = BalExecutionPolicy::new(NonZeroU64::new(3).unwrap());

        assert!(!policy.is_eligible(2));
    }

    #[test]
    fn accepts_transaction_count_equal_to_minimum() {
        let policy = BalExecutionPolicy::new(NonZeroU64::new(3).unwrap());

        assert!(policy.is_eligible(3));
    }

    #[test]
    fn accepts_transaction_count_above_minimum() {
        let policy = BalExecutionPolicy::new(NonZeroU64::new(3).unwrap());

        assert!(policy.is_eligible(4));
    }
}
