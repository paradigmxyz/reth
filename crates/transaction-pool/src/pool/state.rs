bitflags::bitflags! {
    /// Marker to represents the current state of a transaction in the pool and from which the corresponding sub-pool is derived, depending on what bits are set.
    ///
    /// This mirrors [erigon's ephemeral state field](https://github.com/ledgerwatch/erigon/wiki/Transaction-Pool-Design#ordering-function).
    #[derive(Default)]
    pub(crate) struct TxState: u8 {
        /// Set to `1` if all ancestor transactions are pending.
        const NO_PARKED_ANCESTORS = 0b100000;
        /// Set to `1` of the transaction is either the next transaction of the sender (on chain nonce == tx.nonce) or all prior transactions are also present in the pool.
        const NO_NONCE_GAPS = 0b010000;
        /// Bit derived from the sender's balance.
        ///
        /// Set to `1` if the sender's balance can cover the maximum cost for this transaction (`feeCap * gasLimit + value`).
        /// This includes cumulative costs of prior transactions, which ensures that the sender has enough funds for all max cost of prior transactions.
        const ENOUGH_BALANCE = 0b001000;
        /// Bit set to true if the transaction has a lower gas limit than the block's gas limit
        const NOT_TOO_MUCH_GAS = 0b000100;
        /// Covers the Dynamic fee requirement.
        ///
        /// Set to 1 if `feeCap` of the transaction meets the requirement of the pending block.
        const ENOUGH_FEE_CAP_BLOCK = 0b000010;

        const PENDING_POOL_BITS = Self::NO_PARKED_ANCESTORS.bits | Self::NO_NONCE_GAPS.bits | Self::ENOUGH_BALANCE.bits | Self::NOT_TOO_MUCH_GAS.bits |  Self::ENOUGH_FEE_CAP_BLOCK.bits;

        const BASE_FEE_POOL_BITS = Self::NO_PARKED_ANCESTORS.bits | Self::NO_NONCE_GAPS.bits | Self::ENOUGH_BALANCE.bits | Self::NOT_TOO_MUCH_GAS.bits;

        const QUEUED_POOL_BITS  = Self::NO_PARKED_ANCESTORS.bits;

    }
}

// === impl TxState ===

impl TxState {
    /// The state of a transaction is considered `pending`, if the transaction has:
    ///   - _No_ parked ancestors
    ///   - enough balance
    ///   - enough fee cap
    #[inline]
    pub(crate) fn is_pending(&self) -> bool {
        *self >= TxState::PENDING_POOL_BITS
    }

    /// Returns `true` if the transaction has a nonce gap.
    #[inline]
    pub(crate) fn has_nonce_gap(&self) -> bool {
        !self.intersects(TxState::NO_NONCE_GAPS)
    }
}

/// Identifier for the used Sub-pool
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
#[repr(u8)]
pub enum SubPool {
    Queued = 0,
    Pending,
    BaseFee,
}

// === impl PoolDestination ===

impl SubPool {
    /// Whether this transaction is to be moved to the pending sub-pool.
    pub fn is_pending(&self) -> bool {
        matches!(self, SubPool::Pending)
    }

    /// Returns whether this is a promotion depending on the current sub-pool location.
    pub fn is_promoted(&self, other: SubPool) -> bool {
        self > &other
    }
}

impl From<TxState> for SubPool {
    fn from(value: TxState) -> Self {
        if value.is_pending() {
            return SubPool::Pending
        }
        if value < TxState::BASE_FEE_POOL_BITS {
            return SubPool::Queued
        }
        SubPool::BaseFee
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let bits = 0b111110;
        let state = TxState::from_bits(bits).unwrap();
        assert_eq!(SubPool::Pending, state.into());
        assert!(state.is_pending());

        let bits = 0b111110;
        let state = TxState::from_bits(bits).unwrap();
        assert_eq!(SubPool::Pending, state.into());
        assert!(state.is_pending());
    }
}
