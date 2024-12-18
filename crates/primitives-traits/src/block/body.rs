//! Block body abstraction.

use crate::{
    BlockHeader, FullSignedTx, InMemorySize, MaybeSerde, MaybeSerdeBincodeCompat, SignedTransaction,
};
use alloc::{fmt, vec::Vec};
use alloy_consensus::Transaction;
use alloy_eips::{eip2718::Encodable2718, eip4895::Withdrawals};
use alloy_primitives::{Bytes, B256};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullBlockBody: BlockBody<Transaction: FullSignedTx> + MaybeSerdeBincodeCompat {}

impl<T> FullBlockBody for T where T: BlockBody<Transaction: FullSignedTx> + MaybeSerdeBincodeCompat {}

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
}
