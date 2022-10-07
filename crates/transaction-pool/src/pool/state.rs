use std::{
    fmt,
    ops::BitAndAssign,
    sync::atomic::{AtomicU8, Ordering},
};
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


/// Identifier for the used Subpool
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum SubPool {
    Queued = 0,
    Pending,
    Parked,
}

// === impl SubPool ===

impl SubPool {
    /// Converts a u8 into the corresponding variant.
    ///
    /// # Panics
    ///
    /// If `val` does not match any variant
    fn from_u8(val: u8) -> Self {
        match val {
            0 => SubPool::Queued,
            1 => SubPool::Pending,
            2 => SubPool::Parked,
            _ => unreachable!("is shielded; qed"),
        }
    }
}

impl From<TxState> for SubPool {
    fn from(value: TxState) -> Self {
        if value > TxState::BASE_FEE_POOL_BITS {
            return SubPool::Pending
        }
        if value < TxState::BASE_FEE_POOL_BITS {
            return SubPool::Queued
        }
        SubPool::Parked
    }
}

