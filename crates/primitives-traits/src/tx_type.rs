use core::fmt;

use alloy_primitives::{U64, U8};

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
{
}

impl<T> TxType for T where
    T: Send
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
{
}

impl<T> TxType for T where
    T: Send
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
        + TryFrom<u8, Error = Eip2718Error>
        + TryFrom<u64>
        + TryFrom<U64>
        + Encodable
        + Decodable
{
}
