

bitflags::bitflags! {
    /// Marker to assign a transaction into a sub-pool:
    #[derive(Default)]
    pub(crate) struct TxState: u8 {
        const ENOUGH_FEE_CAP_PROTOCOL = 0b100000;
        const NO_NONCE_GAPS = 0b010000;
        const ENOUGH_BALANCE = 0b001000;
        const NOT_TOO_MUCH_GAS = 0b000100;
        const ENOUGH_FEE_CAP_BLOCK = 0b000010;
        const IS_LOCAL = 0b000001;

        const BASE_FEE_POOL_BITS = Self::ENOUGH_FEE_CAP_PROTOCOL.bits + Self::NO_NONCE_GAPS.bits + Self::ENOUGH_BALANCE.bits + Self::NOT_TOO_MUCH_GAS.bits;
        const QUEUED_POOL_BITS  = Self::ENOUGH_FEE_CAP_PROTOCOL.bits;
    }
}

/// Identifier for the used Sub-pool
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum SubPool {
    Queued = 0,
    Pending,
    BaseFee,
}

impl From<TxState> for SubPool {
    fn from(value: TxState) -> Self {
        if value > TxState::BASE_FEE_POOL_BITS {
            return SubPool::Pending
        }
        if value < TxState::BASE_FEE_POOL_BITS {
            return SubPool::Queued
        }
        SubPool::BaseFee
    }
}
