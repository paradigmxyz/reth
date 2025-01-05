//! Block body abstraction.

use crate::{
    BlockHeader, FullSignedTx, InMemorySize, MaybeSerde, MaybeSerdeBincodeCompat, SignedTransaction,
};
use alloc::{fmt, vec::Vec};
use alloy_consensus::{Header, Transaction};
use alloy_eips::{eip2718::Encodable2718, eip4895::Withdrawals};
use alloy_primitives::{Address, Bytes, B256};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullBlockBody: BlockBody<Transaction: FullSignedTx> + MaybeSerdeBincodeCompat {}

impl<T> FullBlockBody for T where T: BlockBody<Transaction: FullSignedTx> + MaybeSerdeBincodeCompat {}

#[cfg(feature = "rayon")]
use rayon::prelude::*;

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
    + 'static
{
    /// Ordered list of signed transactions as committed in block.
    type Transaction: SignedTransaction;

    /// Ommer header type.
    type OmmerHeader: BlockHeader;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &[Self::Transaction];

    /// Returns an iterator over all transaction hashes in the block body.
    fn transaction_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_ {
        self.transactions().iter().map(|tx| tx.tx_hash())
    }

    /// Returns the number of the transactions in the block.
    fn transaction_count(&self) -> usize {
        self.transactions().len()
    }
    /// Consume the block body and return a [`Vec`] of transactions.
    fn into_transactions(self) -> Vec<Self::Transaction>;

    /// Calculate the transaction root for the block body.
    fn calculate_tx_root(&self) -> B256 {
        alloy_consensus::proofs::calculate_transaction_root(self.transactions())
    }

    /// Returns block withdrawals if any.
    fn withdrawals(&self) -> Option<&Withdrawals>;

    /// Calculate the withdrawals root for the block body.
    ///
    /// Returns `None` if there are no withdrawals in the block.
    fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.withdrawals().map(|withdrawals| {
            alloy_consensus::proofs::calculate_withdrawals_root(withdrawals.as_slice())
        })
    }

    /// Returns block ommers if any.
    fn ommers(&self) -> Option<&[Self::OmmerHeader]>;

    /// Calculate the ommers root for the block body.
    ///
    /// Returns `None` if there are no ommers in the block.
    fn calculate_ommers_root(&self) -> Option<B256> {
        self.ommers().map(alloy_consensus::proofs::calculate_ommers_root)
    }

    /// Calculates the total blob gas used by _all_ EIP-4844 transactions in the block.
    fn blob_gas_used(&self) -> u64 {
        self.transactions().iter().filter_map(|tx| tx.blob_gas_used()).sum()
    }

    /// Returns an iterator over all blob versioned hashes in the block body.
    fn blob_versioned_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_ {
        self.transactions().iter().filter_map(|tx| tx.blob_versioned_hashes()).flatten()
    }

    /// Returns an iterator over the encoded 2718 transactions.
    ///
    /// This is also known as `raw transactions`.
    ///
    /// See also [`Encodable2718`].
    #[doc(alias = "raw_transactions_iter")]
    fn encoded_2718_transactions_iter(&self) -> impl Iterator<Item = Vec<u8>> + '_ {
        self.transactions().iter().map(|tx| tx.encoded_2718())
    }

    /// Returns a vector of encoded 2718 transactions.
    ///
    /// This is also known as `raw transactions`.
    ///
    /// See also [`Encodable2718`].
    #[doc(alias = "raw_transactions")]
    fn encoded_2718_transactions(&self) -> Vec<Bytes> {
        self.encoded_2718_transactions_iter().map(Into::into).collect()
    }

    /// Recover signer addresses for all transactions in the block body.
    fn recover_signers(&self) -> Option<Vec<Address>>
    where
        Self::Transaction: SignedTransaction,
    {
        #[cfg(feature = "rayon")]
        {
            self.transactions().into_par_iter().map(|tx| tx.recover_signer()).collect()
        }
        #[cfg(not(feature = "rayon"))]
        {
            self.transactions().iter().map(|tx| tx.recover_signer()).collect()
        }
    }

    /// Recover signer addresses for all transactions in the block body _without ensuring that the
    /// signature has a low `s` value_.
    ///
    /// Returns `None`, if some transaction's signature is invalid.
    fn recover_signers_unchecked(&self) -> Option<Vec<Address>>
    where
        Self::Transaction: SignedTransaction,
    {
        #[cfg(feature = "rayon")]
        {
            self.transactions().into_par_iter().map(|tx| tx.recover_signer_unchecked()).collect()
        }
        #[cfg(not(feature = "rayon"))]
        {
            self.transactions().iter().map(|tx| tx.recover_signer_unchecked()).collect()
        }
    }
}

impl<T> BlockBody for alloy_consensus::BlockBody<T>
where
    T: SignedTransaction,
{
    type Transaction = T;
    type OmmerHeader = Header;

    fn transactions(&self) -> &[Self::Transaction] {
        &self.transactions
    }

    fn into_transactions(self) -> Vec<Self::Transaction> {
        self.transactions
    }

    fn withdrawals(&self) -> Option<&Withdrawals> {
        self.withdrawals.as_ref()
    }

    fn ommers(&self) -> Option<&[Self::OmmerHeader]> {
        Some(&self.ommers)
    }
}

/// This is a helper alias to make it easy to refer to the inner `Transaction` associated type of a
/// given type that implements [`BlockBody`].
pub type BodyTx<N> = <N as BlockBody>::Transaction;

/// This is a helper alias to make it easy to refer to the inner `OmmerHeader` associated type of a
/// given type that implements [`BlockBody`].
pub type BodyOmmer<N> = <N as BlockBody>::OmmerHeader;
