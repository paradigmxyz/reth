//! Block body abstraction.

use alloc::fmt;

use alloy_eips::{eip4895::Withdrawal, eip7685::Requests};
use alloy_primitives::{Address, B256};
use reth_codecs::Compact;

use crate::{
    BlockHeader, FullBlockHeader, FullSignedTx, InMemorySize, SignedTransaction, TransactionExt,
    TxType,
};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlockBody:
    BlockBody<Header: FullBlockHeader, Transaction: FullSignedTx> + Compact
{
}

impl<T> FullBlockBody for T where
    T: BlockBody<Header: FullBlockHeader, Transaction: FullSignedTx> + Compact
{
}

/// Abstraction of block's body.
pub trait BlockBody:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + serde::Serialize
    + for<'de> serde::Deserialize<'de>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + InMemorySize
{
    /// Signed transaction.
    type Transaction: SignedTransaction;

    /// Header type (uncle blocks).
    type Header: BlockHeader;

    /// Withdrawals in block.
    type Withdrawals: IntoIterator<Item = Withdrawal>;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &[Self::Transaction];

    /// Returns `Withdrawals` in the block, if any.
    // todo: branch out into extension trait
    fn withdrawals(&self) -> Option<&Self::Withdrawals>;

    /// Returns reference to uncle block headers.
    fn ommers(&self) -> &[Self::Header];

    /// Returns [`Requests`] in block, if any.
    fn requests(&self) -> Option<&Requests>;

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
        let num_txns = self.transactions().len();
        Self::Transaction::recover_signers(self.transactions(), num_txns)
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
pub trait Body<Header, SignedTx: SignedTransaction, Withdrawals> {
    /// See [`BlockBody`].
    fn transactions(&self) -> &[SignedTx];
    /// See [`BlockBody`].
    fn withdrawals(&self) -> Option<&Withdrawals>;
    /// See [`BlockBody`].
    fn ommers(&self) -> &[Header];
    /// See [`BlockBody`].
    fn requests(&self) -> Option<&Requests>;
    /// See [`BlockBody`].
    fn calculate_tx_root(&self) -> B256;
    /// See [`BlockBody`].
    fn calculate_ommers_root(&self) -> B256;
    /// See [`BlockBody`].
    fn calculate_withdrawals_root(&self) -> Option<B256>;
    /// See [`BlockBody`].
    fn recover_signers(&self) -> Option<Vec<Address>> {
        let num_txns = self.transactions().len();
        SignedTx::recover_signers(self.transactions(), num_txns)
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
