//! Block body abstraction.

use crate::{
    transaction::signed::RecoveryError, BlockHeader, FullSignedTx, InMemorySize, MaybeSerde,
    MaybeSerdeBincodeCompat, SignedTransaction,
};
use alloc::{fmt, vec::Vec};
use alloy_consensus::{Transaction, Typed2718};
use alloy_eips::{eip2718::Encodable2718, eip4895::Withdrawals};
use alloy_primitives::{Address, Bytes, B256};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullBlockBody: BlockBody<Transaction: FullSignedTx> + MaybeSerdeBincodeCompat {}

impl<T> FullBlockBody for T where T: BlockBody<Transaction: FullSignedTx> + MaybeSerdeBincodeCompat {}

/// Abstraction for block's body.
///
/// This type is a container for everything that is included in a block except the header.
/// For ethereum this includes transactions, ommers, and withdrawals.
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
    /// Ordered list of signed transactions as committed in the block.
    type Transaction: SignedTransaction;

    /// Ommer header type.
    type OmmerHeader: BlockHeader;

    /// Returns reference to transactions in the block.
    fn transactions(&self) -> &[Self::Transaction];

    /// A Convenience function to convert this type into the regular ethereum block body that
    /// consists of:
    ///
    /// - Transactions
    /// - Withdrawals
    /// - Ommers
    ///
    /// Note: This conversion can be incomplete. It is not expected that this `Body` is the same as
    /// [`alloy_consensus::BlockBody`] only that it can be converted into it which is useful for
    /// the `eth_` RPC namespace (e.g. RPC block).
    fn into_ethereum_body(self)
        -> alloy_consensus::BlockBody<Self::Transaction, Self::OmmerHeader>;

    /// Returns an iterator over the transactions in the block.
    fn transactions_iter(&self) -> impl Iterator<Item = &Self::Transaction> + '_ {
        self.transactions().iter()
    }

    /// Returns the transaction with the matching hash.
    ///
    /// This is a convenience function for `transactions_iter().find()`
    fn transaction_by_hash(&self, hash: &B256) -> Option<&Self::Transaction> {
        self.transactions_iter().find(|tx| tx.tx_hash() == hash)
    }

    /// Clones the transactions in the block.
    ///
    /// This is a convenience function for `transactions().to_vec()`
    fn clone_transactions(&self) -> Vec<Self::Transaction> {
        self.transactions().to_vec()
    }

    /// Returns an iterator over all transaction hashes in the block body.
    fn transaction_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_ {
        self.transactions_iter().map(|tx| tx.tx_hash())
    }

    /// Returns the number of the transactions in the block.
    fn transaction_count(&self) -> usize {
        self.transactions().len()
    }

    /// Consume the block body and return a [`Vec`] of transactions.
    fn into_transactions(self) -> Vec<Self::Transaction>;

    /// Returns `true` if the block body contains a transaction of the given type.
    fn contains_transaction_type(&self, tx_type: u8) -> bool {
        self.transactions_iter().any(|tx| tx.is_type(tx_type))
    }

    /// Calculate the transaction root for the block body.
    fn calculate_tx_root(&self) -> B256 {
        alloy_consensus::proofs::calculate_transaction_root(self.transactions())
    }

    /// Returns block withdrawals if any.
    fn withdrawals(&self) -> Option<&Withdrawals>;

    /// Calculate the withdrawals root for the block body.
    ///
    /// Returns `RecoveryError` if there are no withdrawals in the block.
    fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.withdrawals().map(|withdrawals| {
            alloy_consensus::proofs::calculate_withdrawals_root(withdrawals.as_slice())
        })
    }

    /// Returns block ommers if any.
    fn ommers(&self) -> Option<&[Self::OmmerHeader]>;

    /// Calculate the ommers root for the block body.
    ///
    /// Returns `RecoveryError` if there are no ommers in the block.
    fn calculate_ommers_root(&self) -> Option<B256> {
        self.ommers().map(alloy_consensus::proofs::calculate_ommers_root)
    }

    /// Calculates the total blob gas used by _all_ EIP-4844 transactions in the block.
    fn blob_gas_used(&self) -> u64 {
        self.transactions_iter().filter_map(|tx| tx.blob_gas_used()).sum()
    }

    /// Returns an iterator over all blob versioned hashes in the block body.
    fn blob_versioned_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_ {
        self.transactions_iter().filter_map(|tx| tx.blob_versioned_hashes()).flatten()
    }

    /// Returns an iterator over the encoded 2718 transactions.
    ///
    /// This is also known as `raw transactions`.
    ///
    /// See also [`Encodable2718`].
    #[doc(alias = "raw_transactions_iter")]
    fn encoded_2718_transactions_iter(&self) -> impl Iterator<Item = Vec<u8>> + '_ {
        self.transactions_iter().map(|tx| tx.encoded_2718())
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
    fn recover_signers(&self) -> Result<Vec<Address>, RecoveryError>
    where
        Self::Transaction: SignedTransaction,
    {
        crate::transaction::recover::recover_signers(self.transactions())
    }

    /// Recover signer addresses for all transactions in the block body.
    ///
    /// Returns an error if some transaction's signature is invalid.
    fn try_recover_signers(&self) -> Result<Vec<Address>, RecoveryError>
    where
        Self::Transaction: SignedTransaction,
    {
        self.recover_signers()
    }

    /// Recover signer addresses for all transactions in the block body _without ensuring that the
    /// signature has a low `s` value_.
    ///
    /// Returns `RecoveryError`, if some transaction's signature is invalid.
    fn recover_signers_unchecked(&self) -> Result<Vec<Address>, RecoveryError>
    where
        Self::Transaction: SignedTransaction,
    {
        crate::transaction::recover::recover_signers_unchecked(self.transactions())
    }

    /// Recover signer addresses for all transactions in the block body _without ensuring that the
    /// signature has a low `s` value_.
    ///
    /// Returns an error if some transaction's signature is invalid.
    fn try_recover_signers_unchecked(&self) -> Result<Vec<Address>, RecoveryError>
    where
        Self::Transaction: SignedTransaction,
    {
        self.recover_signers_unchecked()
    }
}

impl<T, H> BlockBody for alloy_consensus::BlockBody<T, H>
where
    T: SignedTransaction,
    H: BlockHeader,
{
    type Transaction = T;
    type OmmerHeader = H;

    fn transactions(&self) -> &[Self::Transaction] {
        &self.transactions
    }

    fn into_ethereum_body(self) -> Self {
        self
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
