//! Block abstraction.

pub mod body;

use alloc::fmt;
use core::ops;

use alloy_consensus::BlockHeader;
use alloy_primitives::{Address, Sealable, B256};

use crate::{traits::BlockBody, BlockWithSenders, SealedBlock, SealedHeader};

/// Abstraction of block data type.
pub trait Block:
    fmt::Debug
    + Clone
    + PartialEq
    + Eq
    + Default
    + serde::Serialize
    + for<'a> serde::Deserialize<'a>
    + From<(Self::Header, Self::Body)>
    + Into<(Self::Header, Self::Body)>
{
    /// Header part of the block.
    type Header: BlockHeader + Sealable;

    /// The block's body contains the transactions in the block.
    type Body: BlockBody;

    /// A block and block hash.
    type SealedBlock<H = Self::Header, B = Self::Body>;

    /// A block and addresses of senders of transactions in it.
    type BlockWithSenders<T = Self>;

    /// Returns reference to [`BlockHeader`] type.
    fn header(&self) -> &Self::Header;

    /// Returns reference to [`BlockBody`] type.
    fn body(&self) -> &Self::Body;

    /// Calculate the header hash and seal the block so that it can't be changed.
    // todo: can be default impl if sealed block type is made generic over header and body and 
    // migrated to alloy
    fn seal_slow(self) -> Self::SealedBlock;

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    // todo: can be default impl if sealed block type is made generic over header and body and 
    // migrated to alloy
    fn seal(self, hash: B256) -> Self::SealedBlock;

    /// Expensive operation that recovers transaction signer. See
    /// [`SealedBlockWithSenders`](reth_primitives::SealedBlockWithSenders).
    fn senders(&self) -> Option<Vec<Address>> {
        self.body().recover_signers()
    }

    /// Transform into a [`BlockWithSenders`].
    ///
    /// # Panics
    ///
    /// If the number of senders does not match the number of transactions in the block
    /// and the signer recovery for one of the transactions fails.
    ///
    /// Note: this is expected to be called with blocks read from disk.
    #[track_caller]
    fn with_senders_unchecked(self, senders: Vec<Address>) -> Self::BlockWithSenders {
        self.try_with_senders_unchecked(senders).expect("stored block is valid")
    }

    /// Transform into a [`BlockWithSenders`] using the given senders.
    ///
    /// If the number of senders does not match the number of transactions in the block, this falls
    /// back to manually recovery, but _without ensuring that the signature has a low `s` value_.
    /// See also [`TransactionSigned::recover_signer_unchecked`]
    ///
    /// Returns an error if a signature is invalid.
    // todo: can be default impl if block with senders type is made generic over block and migrated 
    // to alloy
    #[track_caller]
    fn try_with_senders_unchecked(
        self,
        senders: Vec<Address>,
    ) -> Result<Self::BlockWithSenders, Self>;

    /// **Expensive**. Transform into a [`BlockWithSenders`] by recovering senders in the contained
    /// transactions.
    ///
    /// Returns `None` if a transaction is invalid.
    // todo: can be default impl if sealed block type is made generic over header and body and 
    // migrated to alloy
    fn with_recovered_senders(self) -> Option<Self::BlockWithSenders>;

    /// Calculates a heuristic for the in-memory size of the [`Block`].
    fn size(&self) -> usize;
}
