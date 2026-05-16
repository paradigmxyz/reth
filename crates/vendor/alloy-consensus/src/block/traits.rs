use super::Block;
use alloy_eips::eip4895::Withdrawals;

/// A trait for ethereum like blocks.
pub trait EthBlock {
    /// Returns reference to withdrawals in the block if present
    fn withdrawals(&self) -> Option<&Withdrawals>;
}

impl<T, H> EthBlock for Block<T, H> {
    fn withdrawals(&self) -> Option<&Withdrawals> {
        self.body.withdrawals.as_ref()
    }
}
