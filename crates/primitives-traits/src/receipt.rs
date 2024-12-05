//! Receipt abstraction

use alloc::vec::Vec;
use core::fmt;

use alloy_consensus::{TxReceipt, Typed2718};
use alloy_primitives::B256;

use crate::{InMemorySize, MaybeArbitrary, MaybeCompact, MaybeSerde};

/// Helper trait that unifies all behaviour required by receipt to support full node operations.
pub trait FullReceipt: Receipt + MaybeCompact {}

impl<T> FullReceipt for T where T: ReceiptExt + MaybeCompact {}

/// Abstraction of a receipt.
#[auto_impl::auto_impl(&, Arc)]
pub trait Receipt:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + TxReceipt<Log = alloy_primitives::Log>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + Typed2718
    + MaybeSerde
    + InMemorySize
    + MaybeArbitrary
{
}

/// Extension if [`Receipt`] used in block execution.
pub trait ReceiptExt: Receipt {
    /// Calculates the receipts root of the given receipts.
    fn receipts_root(receipts: &[&Self]) -> B256;
}

/// Retrieves gas spent by transactions as a vector of tuples (transaction index, gas used).
pub fn gas_spent_by_transactions<I, T>(receipts: I) -> Vec<(u64, u64)>
where
    I: IntoIterator<Item = T>,
    T: TxReceipt,
{
    receipts
        .into_iter()
        .enumerate()
        .map(|(id, receipt)| (id as u64, receipt.cumulative_gas_used() as u64))
        .collect()
}
