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
