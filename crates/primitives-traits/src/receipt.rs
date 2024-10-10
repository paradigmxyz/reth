//! Receipt abstraction

use alloc::fmt;

use alloy_consensus::TxReceipt;
use serde::{Deserialize, Serialize};

/// Abstraction of a receipt.
pub trait Receipt:
    TxReceipt
    + Clone
    + fmt::Debug
    + PartialEq
    + Eq
    + Default
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + Serialize
    + for<'de> Deserialize<'de>
{
    /// Returns transaction type.
    fn tx_type(&self) -> u8;
}
