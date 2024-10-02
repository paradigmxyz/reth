//! Block abstraction.

use alloc::fmt;
use core::{mem, ops};

use alloy_consensus::{BlockHeader, Requests, Transaction, TxType};
use alloy_primitives::{Address, Sealable, B256};
use reth_primitives::{proofs, Withdrawals};

/// Abstraction of block data type.
pub trait Block: From<(Self::Header, Self::Body)> + Into<(Self::Header, Self::Body)> {
    /// Header part of the block.
    type Header: BlockHeader + Sealable;
    /// The block's body contains the transactions in the block.
    type Body: BlockBody;

    /// Returns reference to [`BlockHeader`] type.
    fn header(&self) -> &Self::Header;

    /// Returns reference to [`BlockBody`] type.
    fn body(&self) -> &Self::Body;

    /// Calculate the header hash and seal the block so that it can't be changed.
    fn seal_slow(self) -> SealedBlock {
        let (header, body) = self.into();
        let sealed = header.seal_slow();
        let (header, seal) = sealed.into_parts();
        SealedBlock { header: SealedHeader::new(header, seal), body }
    }

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    fn seal(self, hash: B256) -> SealedBlock {
        let (header, body) = self.into();
        SealedBlock { header: SealedHeader::new(header, hash), body }
    }

    /// Expensive operation that recovers transaction signer. See [`SealedBlockWithSenders`].
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
    fn with_senders_unchecked(self, senders: Vec<Address>) -> BlockWithSenders {
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
    fn try_with_senders_unchecked(self, senders: Vec<Address>) -> Result<BlockWithSenders, Self> {
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
    fn with_recovered_senders(self) -> Option<BlockWithSenders> {
        let senders = self.senders()?;
        Some(BlockWithSenders { block: self, senders })
    }

    /// Calculates a heuristic for the in-memory size of the [`Block`].
    #[inline]
    fn size(&self) -> usize {
        self.header().size() + self.body().size()
    }
}

impl<T> Block for T
where
    T: ops::Deref<Target: Block>
        + From<(<T::Target as Block>::Header, <T::Target as Block>::Body)>
        + Into<(<T::Target as Block>::Header, <T::Target as Block>::Body)>,
{
    type Header = <T::Target as Block>::Header;
    type Body = <T::Target as Block>::Body;

    fn header(&self) -> &Self::Header {
        self.deref().header()
    }

    fn body(&self) -> &Self::Body {
        self.deref().body()
    }
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

    /// Header type (uncle blocks).
    type Header: BlockHeader;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &Vec<Self::SignedTransaction>;

    /// Returns [`Withdrawals`] in the block, if any.
    fn withdrawals(&self) -> Option<&Withdrawals>;

    /// Returns reference to uncle block headers.
    fn ommers(&self) -> &Vec<Self::Header>;

    /// Returns [`Request`] in block, if any.
    fn requests(&self) -> Option<&Requests>;

    /// Create a [`Block`] from the body and its header.
    fn into_block<T: Block<Header = Self::Header, Body = Self>>(self, header: Self::Header) -> T {
        T::from((header, self))
    }

    /// Calculate the transaction root for the block body.
    fn calculate_tx_root(&self) -> B256;

    /// Calculate the ommers root for the block body.
    fn calculate_ommers_root(&self) -> B256;

    /// Calculate the withdrawals root for the block body, if withdrawals exist. If there are no
    /// withdrawals, this will return `None`.
    fn calculate_withdrawals_root(&self) -> Option<B256> {
        Some(proofs::calculate_withdrawals_root(self.withdrawals()?))
    }

    /// Calculate the requests root for the block body, if requests exist. If there are no
    /// requests, this will return `None`.
    fn calculate_requests_root(&self) -> Option<B256> {
        Some(proofs::calculate_requests_root(self.requests()?))
    }

    /// Recover signer addresses for all transactions in the block body.
    fn recover_signers(&self) -> Option<Vec<Address>>;

    /// Returns whether or not the block body contains any blob transactions.
    fn has_blob_transactions(&self) -> bool {
        self.transactions().iter().any(|tx| tx.ty() as u8 == TxType::Eip4844 as u8)
    }

    /// Returns whether or not the block body contains any EIP-7702 transactions.
    fn has_eip7702_transactions(&self) -> bool {
        self.transactions().iter().any(|tx| tx.ty() as u8 == TxType::Eip7702 as u8)
    }

    /// Returns an iterator over all blob transactions of the block
    fn blob_transactions_iter(&self) -> impl Iterator<Item = &Self::SignedTransaction> + '_ {
        self.transactions().iter().filter(|tx| tx.ty() as u8 == TxType::Eip4844 as u8)
    }

    /// Returns only the blob transactions, if any, from the block body.
    fn blob_transactions(&self) -> Vec<&Self::SignedTransaction> {
        self.blob_transactions_iter().collect()
    }

    /// Returns an iterator over all blob versioned hashes from the block body.
    fn blob_versioned_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_;

    /// Returns all blob versioned hashes from the block body.
    fn blob_versioned_hashes(&self) -> Vec<&B256> {
        self.blob_versioned_hashes_iter().collect()
    }

    /// Calculates a heuristic for the in-memory size of the [`BlockBody`].
    fn size(&self) -> usize {
        self.transactions().iter().map(Self::SignedTransaction::size).sum::<usize>() +
            self.transactions().capacity() * mem::size_of::<Self::SignedTransaction>() +
            self.ommers().iter().map(Self::Header::size).sum::<usize>() +
            self.ommers().capacity() * core::mem::size_of::<Self::Header>() +
            self.withdrawals()
                .map_or(mem::size_of::<Option<Withdrawals>>(), Withdrawals::total_size)
    }
}

impl<T> BlockBody for T
where
    T: ops::Deref<Target: BlockBody>
        + Clone
        + fmt::Debug
        + PartialEq
        + Eq
        + Default
        + serde::Serialize
        + for<'de> serde::Deserialize<'de>
        + alloy_rlp::Encodable
        + alloy_rlp::Decodable,
{
    type Header = <T::Target as BlockBody>::Header;
    type SignedTransaction = <T::Target as BlockBody>::SignedTransaction;

    fn transactions(&self) -> &Vec<Self::SignedTransaction> {
        self.deref().transactions()
    }

    fn withdrawals(&self) -> Option<&Withdrawals> {
        self.deref().withdrawals()
    }

    fn ommers(&self) -> &Vec<Self::Header> {
        self.deref().ommers()
    }

    fn requests(&self) -> Option<&Requests> {
        self.deref().requests()
    }

    fn calculate_tx_root(&self) -> B256 {
        self.deref().calculate_tx_root()
    }

    fn calculate_ommers_root(&self) -> B256 {
        self.deref().calculate_ommers_root()
    }

    fn recover_signers(&self) -> Option<Vec<Address>> {
        self.deref().recover_signers()
    }

    fn blob_versioned_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_ {
        self.deref().blob_versioned_hashes_iter()
    }
}
