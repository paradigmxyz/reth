use core::fmt;

use alloy_eips::eip2718::Eip2718Error;
use alloy_primitives::{U64, U8};
use alloy_rlp::{Decodable, Encodable};

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
    + TryFrom<u8, Error = Eip2718Error>
    + TryFrom<u64>
    + TryFrom<U64>
    + Encodable
    + Decodable
{
}
