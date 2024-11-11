//! Receipt abstraction

use core::fmt;

use alloy_consensus::TxReceipt;
use reth_codecs::Compact;
use serde::{Deserialize, Serialize};

/// Helper trait that unifies all behaviour required by receipt to support full node operations.
pub trait FullReceipt: Receipt + Compact {}

impl<T> FullReceipt for T where T: Receipt + Compact {}

/// Abstraction of a receipt.
pub trait Receipt:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + TxReceipt
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + Serialize
    + for<'de> Deserialize<'de>
{
    /// Returns transaction type.
    fn tx_type(&self) -> u8;
}
