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
    + TryFrom<u8>
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
}
