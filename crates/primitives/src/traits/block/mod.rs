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

    /// Returns reference to [`BlockHeader`] type.
    fn header(&self) -> &Self::Header;

    /// Returns reference to [`BlockBody`] type.
    fn body(&self) -> &Self::Body;

    /// Calculate the header hash and seal the block so that it can't be changed.
    fn seal_slow(self) -> SealedBlock<Self::Header, Self::Body> {
        let (header, body) = self.into();
        let sealed = header.seal_slow();
        let (header, seal) = sealed.into_parts();
        SealedBlock { header: SealedHeader::new(header, seal), body }
    }

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    fn seal(self, hash: B256) -> SealedBlock<Self::Header, Self::Body> {
        let (header, body) = self.into();
        SealedBlock { header: SealedHeader::new(header, hash), body }
    }

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
    fn with_senders_unchecked(self, senders: Vec<Address>) -> BlockWithSenders<Self> {
        self.try_with_senders_unchecked(senders).expect("stored block is valid")
    }

    /// Transform into a [`BlockWithSenders`] using the given senders.
    ///
    /// If the number of senders does not match the number of transactions in the block, this falls
    /// back to manually recovery, but _without ensuring that the signature has a low `s` value_.
    /// See also [`TransactionSigned::recover_signer_unchecked`]
    ///
    /// Returns an error if a signature is invalid.
    #[track_caller]
    fn try_with_senders_unchecked(
        self,
        senders: Vec<Address>,
    ) -> Result<BlockWithSenders<Self>, Self> {
        let senders = if self.body().transactions().len() == senders.len() {
            senders
        } else {
            let Some(senders) = self.body().recover_signers() else { return Err(self) };
            senders
        };

        Ok(BlockWithSenders { block: self, senders })
    }

    /// **Expensive**. Transform into a [`BlockWithSenders`] by recovering senders in the contained
    /// transactions.
    ///
    /// Returns `None` if a transaction is invalid.
    fn with_recovered_senders(self) -> Option<BlockWithSenders<Self>> {
        let senders = self.senders()?;
        Some(BlockWithSenders { block: self, senders })
    }

    /// Calculates a heuristic for the in-memory size of the [`Block`].
    fn size(&self) -> usize;
}

impl<T> Block for T
where
    T: ops::Deref<Target: Block>
        + fmt::Debug
        + Clone
        + PartialEq
        + Eq
        + Default
        + serde::Serialize
        + for<'a> serde::Deserialize<'a>
        + From<(<T::Target as Block>::Header, <T::Target as Block>::Body)>
        + Into<(<T::Target as Block>::Header, <T::Target as Block>::Body)>,
{
    type Header = <T::Target as Block>::Header;
    type Body = <T::Target as Block>::Body;

    #[inline]
    fn header(&self) -> &Self::Header {
        self.deref().header()
    }

    #[inline]
    fn body(&self) -> &Self::Body {
        self.deref().body()
    }

    #[inline]
    fn size(&self) -> usize {
        self.deref().size()
    }
}
