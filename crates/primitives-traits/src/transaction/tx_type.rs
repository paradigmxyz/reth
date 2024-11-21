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
