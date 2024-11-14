//! Block body abstraction.

use alloc::fmt;

use reth_codecs::Compact;

use crate::{FullSignedTx, InMemorySize, SignedTransaction};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullBlockBody: BlockBody<Transaction: FullSignedTx> + Compact {}

impl<T> FullBlockBody for T where T: BlockBody<Transaction: FullSignedTx> + Compact {}

/// Abstraction for block's body.
pub trait BlockBody:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + serde::Serialize
    + for<'de> serde::Deserialize<'de>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + InMemorySize
{
    /// Signed transaction, committed in block.
    type Transaction: SignedTransaction;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &[Self::Transaction];
}
