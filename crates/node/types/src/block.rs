//! Block abstraction.

use alloc::fmt;

use alloy_consensus::{BlockHeader, Transaction};

/// Abstraction of block data type.
pub trait Block {
    /// Header part of the block.
    type Header: BlockHeader;
    /// The block's body contains the transactions in the block.
    type Body: BlockBody;
}

/// Abstraction for block's body.
pub trait BlockBody:
    Clone
    + fmt::Debug
    + PartialEq
    + Eq
    + Default
    + serde::Serialize
    + for<'de> serde::Deserialize<'de>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
{
    /// Ordered list of signed transactions as committed in block.
    // todo: requires trait for signed transaction
    type SignedTransaction: Transaction;
}
