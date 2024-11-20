//! Abstraction of transaction envelope type ID.

use core::fmt;

use alloy_primitives::{U64, U8};
use reth_codecs::Compact;

use crate::InMemorySize;

/// Helper trait that unifies all behaviour required by transaction type ID to support full node
/// operations.
pub trait FullTxType: TxType + Compact {}

impl<T> FullTxType for T where T: TxType + Compact {}

/// Trait representing the behavior of a transaction type.
pub trait TxType:
    Send
    + Sync
    + Unpin
    + Clone
    + Copy
    + Default
    + fmt::Debug
    + fmt::Display
    + PartialEq
    + Eq
    + PartialEq<u8>
    + Into<u8>
    + Into<U8>
    + TryFrom<u8, Error: fmt::Debug>
    + TryFrom<u64, Error: fmt::Debug>
    + TryFrom<U64>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + InMemorySize
{
    /// Returns `true` if this is a legacy transaction.
    fn is_legacy(&self) -> bool;

    /// Returns `true` if this is an eip-2930 transaction.
    fn is_eip2930(&self) -> bool;

    /// Returns `true` if this is an eip-1559 transaction.
    fn is_eip1559(&self) -> bool;

    /// Returns `true` if this is an eip-4844 transaction.
    fn is_eip4844(&self) -> bool;

    /// Returns `true` if this is an eip-7702 transaction.
    fn is_eip7702(&self) -> bool;
}
