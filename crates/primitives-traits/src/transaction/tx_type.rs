//! Abstraction of transaction envelope type ID.

use crate::{InMemorySize, MaybeArbitrary, MaybeCompact};
use alloy_consensus::Typed2718;
use alloy_primitives::{U64, U8};
use core::fmt;

/// Helper trait that unifies all behaviour required by transaction type ID to support full node
/// operations.
pub trait FullTxType: TxType + MaybeCompact {}

impl<T> FullTxType for T where T: TxType + MaybeCompact {}

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
    + Typed2718
    + InMemorySize
    + MaybeArbitrary
{
    /// Returns whether this transaction type can be __broadcasted__ as full transaction over the
    /// network.
    ///
    /// Some transactions are not broadcastable as objects and only allowed to be broadcasted as
    /// hashes, e.g. because they missing context (e.g. blob sidecar).
    fn is_broadcastable_in_full(&self) -> bool {
        // EIP-4844 transactions are not broadcastable in full, only hashes are allowed.
        !self.is_eip4844()
    }
}

#[cfg(feature = "op")]
impl TxType for op_alloy_consensus::OpTxType {}

impl TxType for alloy_consensus::TxType {}
