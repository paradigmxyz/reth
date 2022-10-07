use crate::pool::txpool2::SubPool;
use std::{
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

/// An atomic that contains the `TxState` bitfield
pub(crate) struct AtomicTxState(AtomicU8);

// === impl AtomicTxSate ===
impl AtomicTxState {
    #[inline]
    fn get_bit(&self, bit: usize, ord: Ordering) -> bool {
        self.0.load(ord) & (1 << bit) != 0
    }

    /// Sets a all the bits in `other`.
    #[inline]
    pub(crate) fn set_bit(&self, other: TxState) {
        self.0.fetch_or(other.bits, Ordering::Relaxed);
    }

    #[inline]
    fn clear_bit(&self, bit: usize, ord: Ordering) {
        self.0.fetch_and(!(1 << bit), ord);
    }

    /// Sets the entire value state value.
    #[inline]
    pub(crate) fn set(&self, state: TxState) {
        self.0.store(state.bits, Ordering::Relaxed)
    }

    /// Sets the new value and returns the old.
    #[inline]
    pub fn swap(&self, state: TxState) -> TxState {
        let bits = self.0.swap(state.bits, Ordering::Relaxed);
        TxState { bits }
    }

    /// Returns the currently held `SubPool` value.
    #[inline]
    pub fn get(&self) -> TxState {
        let bits = self.0.load(Ordering::Relaxed);
        TxState { bits }
    }
}

impl BitAndAssign<TxState> for AtomicTxState {
    #[inline]
    fn bitand_assign(&mut self, other: TxState) {}
}
