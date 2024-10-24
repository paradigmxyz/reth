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
    + From<alloy_consensus::TxType>
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
}
