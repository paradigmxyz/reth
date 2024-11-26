//! Block body abstraction.

use alloc::{fmt, vec::Vec};

use alloy_consensus::Transaction;

use crate::{FullSignedTx, InMemorySize, MaybeArbitrary, MaybeSerde};

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
    /// Ordered list of signed transactions as committed in block.
    type Transaction: Transaction;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &[Self::Transaction];

    /// Consume the block body and return a [`Vec`] of transactions.
    fn into_transactions(self) -> Vec<Self::Transaction>;
}
