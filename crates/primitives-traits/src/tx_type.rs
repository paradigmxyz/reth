use alloy_eips::eip2718::Eip2718Error;
use alloy_primitives::{U64, U8};
use alloy_rlp::{Decodable, Encodable};
use core::fmt::{Debug, Display};

/// Trait representing the behavior of a transaction type.
pub trait TxType:
    Into<u8>
    + Into<U8>
    + PartialEq
    + Eq
    + PartialEq<u8>
    + TryFrom<u8, Error = Eip2718Error>
    + TryFrom<u64>
    + TryFrom<U64>
    + Debug
    + Display
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
