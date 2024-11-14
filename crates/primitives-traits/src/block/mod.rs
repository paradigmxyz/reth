//! Block abstraction.

pub mod body;
pub mod header;

use alloc::fmt;

use alloy_eips::eip7685::Requests;
use alloy_primitives::{Address, B256};
use reth_codecs::Compact;

use crate::{BlockBody, BlockHeader, Body, FullBlockBody, FullBlockHeader, InMemorySize};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlock: Block<Header: FullBlockHeader, Body: FullBlockBody> + Compact {}

impl<T> FullBlock for T where T: Block<Header: FullBlockHeader, Body: FullBlockBody> + Compact {}

/// Abstraction of block data type.
// todo: make sealable super-trait, depends on <https://github.com/paradigmxyz/reth/issues/11449>
// todo: make with senders extension trait, so block can be impl by block type already containing
// senders
pub trait Block:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + serde::Serialize
    + for<'a> serde::Deserialize<'a>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + alloy_consensus::BlockHeader
    + Body<
        Self::Header,
        <Self::Body as BlockBody>::SignedTransaction,
        <Self::Body as BlockBody>::Withdrawals,
    > + InMemorySize
{
    /// Header part of the block.
    type Header: BlockHeader;

    /// The block's body contains the transactions in the block.
    type Body: BlockBody<Header = Self::Header>;

    /// Returns reference to block header.
    fn header(&self) -> &Self::Header;

    /// Returns reference to block body.
    fn body(&self) -> &Self::Body;
}

impl<T: Block>
    Body<T::Header, <T::Body as BlockBody>::SignedTransaction, <T::Body as BlockBody>::Withdrawals>
    for T
{
    fn transactions(&self) -> &[<T::Body as BlockBody>::SignedTransaction] {
        self.body().transactions()
    }

    fn withdrawals(&self) -> Option<&<T::Body as BlockBody>::Withdrawals> {
        self.body().withdrawals()
    }

    fn ommers(&self) -> &[T::Header] {
        self.body().ommers()
    }

    fn requests(&self) -> Option<&Requests> {
        self.body().requests()
    }

    fn calculate_tx_root(&self) -> B256 {
        self.body().calculate_tx_root()
    }

    fn calculate_ommers_root(&self) -> B256 {
        self.body().calculate_ommers_root()
    }

    fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.body().calculate_withdrawals_root()
    }

    fn recover_signers(&self) -> Option<Vec<Address>> {
        self.body().recover_signers()
    }

    fn blob_versioned_hashes(&self) -> Vec<&B256> {
        self.body().blob_versioned_hashes()
    }

    fn blob_versioned_hashes_copied(&self) -> Vec<B256> {
        self.body().blob_versioned_hashes_copied()
    }
}
