bitflags::bitflags! {
    /// Marker to represents the current state of a transaction in the pool and from which the corresponding sub-pool is derived, depending on what bits are set.
    ///
    /// This mirrors [erigon's ephemeral state field](https://github.com/ledgerwatch/erigon/wiki/Transaction-Pool-Design#ordering-function).
    ///
    /// The [SubPool] the transaction belongs to is derived from its state and determined by the following sequential checks:
    ///
    /// - If it satisfies the [TxState::PENDING_POOL_BITS] it belongs in the pending sub-pool: [SubPool::Pending].
    /// - If it is an EIP-4844 blob transaction it belongs in the blob sub-pool: [SubPool::Blob].
    /// - If it satisfies the [TxState::BASE_FEE_POOL_BITS] it belongs in the base fee sub-pool: [SubPool::BaseFee].
    ///
    /// Otherwise, it belongs in the queued sub-pool: [SubPool::Queued].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
    pub(crate) struct TxState: u8 {
        /// Set to `1` if all ancestor transactions are pending.
        const NO_PARKED_ANCESTORS = 0b10000000;
        /// Set to `1` of the transaction is either the next transaction of the sender (on chain nonce == tx.nonce) or all prior transactions are also present in the pool.
        const NO_NONCE_GAPS = 0b01000000;
        /// Bit derived from the sender's balance.
        ///
        /// Set to `1` if the sender's balance can cover the maximum cost for this transaction (`feeCap * gasLimit + value`).
        /// This includes cumulative costs of prior transactions, which ensures that the sender has enough funds for all max cost of prior transactions.
        const ENOUGH_BALANCE = 0b00100000;
        /// Bit set to true if the transaction has a lower gas limit than the block's gas limit.
        const NOT_TOO_MUCH_GAS = 0b00010000;
        /// Covers the Dynamic fee requirement.
        ///
        /// Set to 1 if `maxFeePerGas` of the transaction meets the requirement of the pending block.
        const ENOUGH_FEE_CAP_BLOCK = 0b00001000;
        /// Covers the dynamic blob fee requirement, only relevant for EIP-4844 blob transactions.
        ///
        /// Set to 1 if `maxBlobFeePerGas` of the transaction meets the requirement of the pending block.
        const ENOUGH_BLOB_FEE_CAP_BLOCK = 0b00000100;
        /// Marks whether the transaction is a blob transaction.
        ///
        /// We track this as part of the state for simplicity, since blob transactions are handled differently and are mutually exclusive with normal transactions.
        const BLOB_TRANSACTION = 0b00000010;

        const PENDING_POOL_BITS = Self::NO_PARKED_ANCESTORS.bits() | Self::NO_NONCE_GAPS.bits() | Self::ENOUGH_BALANCE.bits() | Self::NOT_TOO_MUCH_GAS.bits() |  Self::ENOUGH_FEE_CAP_BLOCK.bits() | Self::ENOUGH_BLOB_FEE_CAP_BLOCK.bits();

        const BASE_FEE_POOL_BITS = Self::NO_PARKED_ANCESTORS.bits() | Self::NO_NONCE_GAPS.bits() | Self::ENOUGH_BALANCE.bits() | Self::NOT_TOO_MUCH_GAS.bits();

        const QUEUED_POOL_BITS  = Self::NO_PARKED_ANCESTORS.bits();

        const BLOB_POOL_BITS  = Self::BLOB_TRANSACTION.bits();
    }
}

impl TxState {
    /// The state of a transaction is considered `pending`, if the transaction has:
    ///   - _No_ parked ancestors
    ///   - enough balance
    ///   - enough fee cap
    ///   - enough blob fee cap
    #[inline]
    pub(crate) const fn is_pending(&self) -> bool {
        self.bits() >= Self::PENDING_POOL_BITS.bits()
    }

    /// Whether this transaction is a blob transaction.
    #[inline]
    pub(crate) const fn is_blob(&self) -> bool {
        self.contains(Self::BLOB_TRANSACTION)
    }

    /// Returns `true` if the transaction has a nonce gap.
    #[inline]
    pub(crate) const fn has_nonce_gap(&self) -> bool {
        !self.intersects(Self::NO_NONCE_GAPS)
    }
}

/// Identifier for the transaction Sub-pool
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(u8)]
pub enum SubPool {
    /// The queued sub-pool contains transactions that are not ready to be included in the next
    /// block because they have missing or queued ancestors or the sender the lacks funds to
    /// execute this transaction.
    Queued = 0,
    /// The base-fee sub-pool contains transactions that are not ready to be included in the next
    /// block because they don't meet the base fee requirement.
    BaseFee,
    /// The blob sub-pool contains all blob transactions that are __not__ pending.
    Blob,
    /// The pending sub-pool contains transactions that are ready to be included in the next block.
    Pending,
}

impl SubPool {
    /// Whether this transaction is to be moved to the pending sub-pool.
    #[inline]
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    /// Whether this transaction is in the queued pool.
    #[inline]
    pub const fn is_queued(&self) -> bool {
        matches!(self, Self::Queued)
    }

    /// Whether this transaction is in the base fee pool.
    #[inline]
    pub const fn is_base_fee(&self) -> bool {
        matches!(self, Self::BaseFee)
    }

    /// Whether this transaction is in the blob pool.
    #[inline]
    pub const fn is_blob(&self) -> bool {
        matches!(self, Self::Blob)
    }

    /// Returns whether this is a promotion depending on the current sub-pool location.
    #[inline]
    pub fn is_promoted(&self, other: Self) -> bool {
        self > &other
    }
}

impl From<TxState> for SubPool {
    fn from(value: TxState) -> Self {
        if value.is_pending() {
            Self::Pending
        } else if value.is_blob() {
            // all _non-pending_ blob transactions are in the blob sub-pool
            Self::Blob
        } else if value.bits() < TxState::BASE_FEE_POOL_BITS.bits() {
            Self::Queued
        } else {
            Self::BaseFee
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_promoted() {
        assert!(SubPool::BaseFee.is_promoted(SubPool::Queued));
        assert!(SubPool::Pending.is_promoted(SubPool::BaseFee));
        assert!(SubPool::Pending.is_promoted(SubPool::Queued));
        assert!(SubPool::Pending.is_promoted(SubPool::Blob));
        assert!(!SubPool::BaseFee.is_promoted(SubPool::Pending));
        assert!(!SubPool::Queued.is_promoted(SubPool::BaseFee));
    }

    #[test]
    fn test_tx_state() {
        let mut state = TxState::default();
        state |= TxState::NO_NONCE_GAPS;
        assert!(state.intersects(TxState::NO_NONCE_GAPS))
    }

    #[test]
    fn test_tx_queued() {
        let state = TxState::default();
        assert_eq!(SubPool::Queued, state.into());

        let state = TxState::NO_PARKED_ANCESTORS |
            TxState::NO_NONCE_GAPS |
            TxState::NOT_TOO_MUCH_GAS |
            TxState::ENOUGH_FEE_CAP_BLOCK;
        assert_eq!(SubPool::Queued, state.into());
    }

    #[test]
    fn test_tx_pending() {
        let state = TxState::PENDING_POOL_BITS;
        assert_eq!(SubPool::Pending, state.into());
        assert!(state.is_pending());

        let bits = 0b11111100;
        let state = TxState::from_bits(bits).unwrap();
        assert_eq!(SubPool::Pending, state.into());
        assert!(state.is_pending());

        let bits = 0b11111110;
        let state = TxState::from_bits(bits).unwrap();
        assert_eq!(SubPool::Pending, state.into());
        assert!(state.is_pending());
    }

    #[test]
    fn test_blob() {
        let mut state = TxState::PENDING_POOL_BITS;
        state.insert(TxState::BLOB_TRANSACTION);
        assert!(state.is_pending());

        state.remove(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK);
        assert!(state.is_blob());
        assert!(!state.is_pending());

        state.insert(TxState::ENOUGH_BLOB_FEE_CAP_BLOCK);
        state.remove(TxState::ENOUGH_FEE_CAP_BLOCK);
        assert!(state.is_blob());
        assert!(!state.is_pending());
    }

    #[test]
    fn test_tx_state_no_nonce_gap() {
        let mut state = TxState::default();
        state |= TxState::NO_NONCE_GAPS;
        assert!(!state.has_nonce_gap());
    }

    #[test]
    fn test_tx_state_with_nonce_gap() {
        let state = TxState::default();
        assert!(state.has_nonce_gap());
    }

    #[test]
    fn test_tx_state_enough_balance() {
        let mut state = TxState::default();
        state.insert(TxState::ENOUGH_BALANCE);
        assert!(state.contains(TxState::ENOUGH_BALANCE));
    }

    #[test]
    fn test_tx_state_not_too_much_gas() {
        let mut state = TxState::default();
        state.insert(TxState::NOT_TOO_MUCH_GAS);
        assert!(state.contains(TxState::NOT_TOO_MUCH_GAS));
    }

    #[test]
    fn test_tx_state_enough_fee_cap_block() {
        let mut state = TxState::default();
        state.insert(TxState::ENOUGH_FEE_CAP_BLOCK);
        assert!(state.contains(TxState::ENOUGH_FEE_CAP_BLOCK));
    }

    #[test]
    fn test_tx_base_fee() {
        let state = TxState::BASE_FEE_POOL_BITS;
        assert_eq!(SubPool::BaseFee, state.into());
    }

    #[test]
    fn test_blob_transaction_only() {
        let state = TxState::BLOB_TRANSACTION;
        assert_eq!(SubPool::Blob, state.into());
        assert!(state.is_blob());
        assert!(!state.is_pending());
    }

    #[test]
    fn test_blob_transaction_with_base_fee_bits() {
        let mut state = TxState::BASE_FEE_POOL_BITS;
        state.insert(TxState::BLOB_TRANSACTION);
        assert_eq!(SubPool::Blob, state.into());
        assert!(state.is_blob());
        assert!(!state.is_pending());
    }
}
