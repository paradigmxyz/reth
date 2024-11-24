//! Block body abstraction.

use alloc::vec::Vec;
use core::fmt;
#[cfg(feature = "std")]
use std::sync::LazyLock;

use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{Address, B256};
use once_cell as _;
#[cfg(not(feature = "std"))]
use once_cell::sync::Lazy as LazyLock;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::{
    Block, FullSignedTx, InMemorySize, MaybeArbitrary, MaybeSerde, SignedTransaction, TxType,
};

/// Expected number of transactions where we can expect a speed-up by recovering the senders in
/// parallel.
pub static PARALLEL_SENDER_RECOVERY_THRESHOLD: LazyLock<usize> =
    LazyLock::new(|| match rayon::current_num_threads() {
        0..=1 => usize::MAX,
        2..=8 => 10,
        _ => 5,
    });

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlockBody: BlockBody<Transaction: FullSignedTx> {}

impl<T> FullBlockBody for T where T: BlockBody<Transaction: FullSignedTx> {}

/// Abstraction of block's body.
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
    + MaybeArbitrary
{
    /// Signed transaction.
    type Transaction: SignedTransaction + 'static;

    // todo
    // /// Header type (uncle blocks).
    // type Header: BlockHeader + 'static;

    /// Withdrawals in block.
    type Withdrawals: IntoIterator<Item = Withdrawal> + 'static;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &Vec<Self::Transaction>;

    // todo
    // /// Returns reference to uncle block headers.
    // fn ommers(&self) -> &[Self::Header];

    /// Returns `Withdrawals` in the block, if any.
    // todo: branch out into extension trait
    fn withdrawals(&self) -> Option<&Self::Withdrawals>;

    /// Calculate the transaction root for the block body.
    fn calculate_tx_root(&self) -> B256;

    /// Calculate the ommers root for the block body.
    fn calculate_ommers_root(&self) -> B256;

    /// Calculate the withdrawals root for the block body, if withdrawals exist. If there are no
    /// withdrawals, this will return `None`.
    // todo: can be default impl if `calculate_withdrawals_root` made into a method on
    // `Withdrawals` and `Withdrawals` moved to alloy
    fn calculate_withdrawals_root(&self) -> Option<B256>;

    /// Recover signer addresses for all transactions in the block body.
    fn recover_signers(&self) -> Option<Vec<Address>> {
        if self.transactions().len() < *PARALLEL_SENDER_RECOVERY_THRESHOLD {
            self.transactions().iter().map(|tx| tx.recover_signer()).collect()
        } else {
            self.transactions().par_iter().map(|tx| tx.recover_signer()).collect()
        }
    }

    /// Returns whether or not the block body contains any blob transactions.
    fn has_blob_transactions(&self) -> bool {
        self.transactions().iter().any(|tx| tx.tx_type().is_eip4844())
    }

    /// Returns whether or not the block body contains any EIP-7702 transactions.
    fn has_eip7702_transactions(&self) -> bool {
        self.transactions().iter().any(|tx| tx.tx_type().is_eip7702())
    }

    /// Returns an iterator over all blob transactions of the block
    fn blob_transactions_iter(&self) -> impl Iterator<Item = &Self::Transaction> {
        self.transactions().iter().filter(|tx| tx.tx_type().is_eip4844())
    }

    /// Returns only the blob transactions, if any, from the block body.
    fn blob_transactions(&self) -> Vec<&Self::Transaction> {
        self.blob_transactions_iter().collect()
    }

    /// Returns references to all blob versioned hashes from the block body.
    fn blob_versioned_hashes(&self) -> Vec<&B256>;

    /// Returns all blob versioned hashes from the block body.
    fn blob_versioned_hashes_copied(&self) -> Vec<B256>;
}

/// Helper trait to implement [`BlockBody`] functionality for [`Block`](crate::Block) types.
pub trait Body<Header: 'static, SignedTx: SignedTransaction + 'static, Withdrawals: 'static> {
    /// See [`BlockBody`].
    fn transactions(&self) -> &[SignedTx];

    /// See [`BlockBody`].
    fn withdrawals(&self) -> Option<&Withdrawals>;

    // todo
    // /// See [`BlockBody`].
    //fn ommers(&self) -> &[Header];

    /// See [`BlockBody`].
    fn calculate_tx_root(&self) -> B256;

    /// See [`BlockBody`].
    fn calculate_ommers_root(&self) -> B256;

    /// See [`BlockBody`].
    fn calculate_withdrawals_root(&self) -> Option<B256>;

    /// See [`BlockBody`].
    fn recover_signers(&self) -> Option<Vec<Address>> {
        if self.transactions().len() < *PARALLEL_SENDER_RECOVERY_THRESHOLD {
            self.transactions().iter().map(|tx| tx.recover_signer()).collect()
        } else {
            self.transactions().par_iter().map(|tx| tx.recover_signer()).collect()
        }
    }

    /// See [`BlockBody`].
    fn has_blob_transactions(&self) -> bool {
        self.transactions().iter().any(|tx| tx.tx_type().is_eip4844())
    }

    /// See [`BlockBody`].
    fn has_eip7702_transactions(&self) -> bool {
        self.transactions().iter().any(|tx| tx.tx_type().is_eip7702())
    }

    /// See [`BlockBody`].
    fn blob_transactions_iter<'a>(&'a self) -> impl Iterator<Item = &'a SignedTx> + 'a
    where
        SignedTx: 'a,
    {
        self.transactions().iter().filter(|tx| tx.tx_type().is_eip4844())
    }

    /// See [`BlockBody`].
    fn blob_transactions(&self) -> Vec<&SignedTx> {
        self.blob_transactions_iter().collect()
    }

    /// See [`BlockBody`].
    fn blob_versioned_hashes(&self) -> Vec<&B256>;

    /// See [`BlockBody`].
    fn blob_versioned_hashes_copied(&self) -> Vec<B256>;
}

impl<T: Block>
    Body<T::Header, <T::Body as BlockBody>::Transaction, <T::Body as BlockBody>::Withdrawals>
    for T
{
    #[inline]
    fn transactions(&self) -> &[<T::Body as BlockBody>::Transaction] {
        self.body().transactions()
    }

    #[inline]
    fn withdrawals(&self) -> Option<&<T::Body as BlockBody>::Withdrawals> {
        self.body().withdrawals()
    }

    // todo
    //#[inline]
    // fn ommers(&self) -> &[T::Header] {
    //    self.body().ommers()
    //}

    #[inline]
    fn calculate_tx_root(&self) -> B256 {
        self.body().calculate_tx_root()
    }

    #[inline]
    fn calculate_ommers_root(&self) -> B256 {
        self.body().calculate_ommers_root()
    }

    #[inline]
    fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.body().calculate_withdrawals_root()
    }

    #[inline]
    fn recover_signers(&self) -> Option<Vec<Address>> {
        self.body().recover_signers()
    }

    #[inline]
    fn blob_versioned_hashes(&self) -> Vec<&B256> {
        self.body().blob_versioned_hashes()
    }

    #[inline]
    fn blob_versioned_hashes_copied(&self) -> Vec<B256> {
        self.body().blob_versioned_hashes_copied()
    }
}
