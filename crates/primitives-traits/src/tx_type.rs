use core::fmt;

use alloy_primitives::{U64, U8};
use alloy_rlp::{Decodable, Encodable};

/// Trait representing the behavior of a transaction type.
pub trait TxType:
    Into<u8>
    + Into<U8>
    + PartialEq
    + Eq
    + PartialEq<u8>
    + TryFrom<u8, Error: fmt::Debug>
    + TryFrom<u64, Error: fmt::Debug>
    + TryFrom<U64, Error: fmt::Debug>
    + fmt::Debug
    + fmt::Display
    + Clone
    + Copy
    + Default
    + Encodable
    + Decodable
    + Send
    + Sync
    + 'static
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

impl<T> TxType for T where
    T: Into<u8>
        + Into<U8>
        + PartialEq
        + Eq
        + PartialEq<u8>
        + TryFrom<u8, Error: fmt::Debug>
        + TryFrom<u64, Error: fmt::Debug>
        + TryFrom<U64, Error: fmt::Debug>
        + fmt::Debug
        + fmt::Display
        + Clone
        + Copy
        + Default
        + Encodable
        + Decodable
        + Send
        + Sync
        + 'static
{
}
