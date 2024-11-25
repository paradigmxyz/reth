//! Block body abstraction.

use alloc::{fmt, vec::Vec};

use crate::{FullSignedTx, InMemorySize, MaybeArbitrary, MaybeSerde, SignedTransaction};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullBlockBody: BlockBody<Transaction: FullSignedTx> {}

impl<T> FullBlockBody for T where T: BlockBody<Transaction: FullSignedTx> {}

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
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + InMemorySize
    + MaybeSerde
    + MaybeArbitrary
{
    /// Signed transaction, committed in block.
    type Transaction: SignedTransaction;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &[Self::Transaction];

    /// Consume the block body and return a [`Vec`] of transactions.
    fn into_transactions(self) -> Vec<Self::Transaction>;
}
