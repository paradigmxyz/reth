//! Block body abstraction.

use alloc::fmt;

use alloy_consensus::Transaction;

use crate::{FullBlockHeader, FullSignedTx, InMemorySize, MaybeSerde};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullBlockBody: BlockBody<Header: FullBlockHeader, Transaction: FullSignedTx> {}

impl<T> FullBlockBody for T where T: BlockBody<Header: FullBlockHeader, Transaction: FullSignedTx> {}

/// Abstraction for block's body.
#[auto_impl::auto_impl(&, Arc)]
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
{
    /// Uncle block header.
    type Header: 'static;

    /// Ordered list of signed transactions as committed in block.
    // todo: requires trait for signed transaction
    type Transaction: Transaction;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &[Self::Transaction];

    /// Returns slice of uncle block headers.
    fn ommers(&self) -> &[Self::Header];
}
