//! Block body abstraction.

use alloc::{fmt, vec::Vec};

use alloy_eips::{eip4895::Withdrawal, eip7685::Requests};
use alloy_primitives::{Address, B256};
use reth_codecs::Compact;

use crate::{
    AlloyTransactionExt, Block, BlockHeader, FullBlockHeader, FullSignedTx, SignedTransaction,
    TxType,
};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlockBody:
    BlockBody<Ommer: FullBlockHeader, SignedTransaction: FullSignedTx> + Compact
{
}

impl<T> FullBlockBody for T where
    T: BlockBody<Ommer: FullBlockHeader, SignedTransaction: FullSignedTx> + Compact
{
}

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
    + serde::Serialize
    + for<'de> serde::Deserialize<'de>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + Body
{
    /// Create a [`Block`] from the body and its header.
    fn into_block<T: Block<Header = Self::Ommer, Body = Self>>(self, header: Self::Ommer) -> T {
        T::from((header, self))
    }
}

/// Block body functionality.
pub trait Body {
    /// Signed transaction.
    type SignedTransaction: SignedTransaction;

    /// Header type (uncle blocks).
    type Ommer: BlockHeader;

    /// Withdrawals in block.
    type Withdrawals: Iterator<Item = Withdrawal>;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &[Self::SignedTransaction];

    /// Returns `Withdrawals` in the block, if any.
    // todo: branch out into extension trait
    fn withdrawals(&self) -> Option<&Self::Withdrawals>;

    /// Returns reference to uncle block headers.
    fn ommers(&self) -> &[Self::Ommer];

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
    fn recover_signers(&self) -> Option<Vec<Address>>;

    /// Returns whether or not the block body contains any blob transactions.
    fn has_blob_transactions(&self) -> bool {
        self.transactions().iter().any(|tx| tx.tx_type().is_eip4844())
    }

    /// Returns whether or not the block body contains any EIP-7702 transactions.
    fn has_eip7702_transactions(&self) -> bool {
        self.transactions().iter().any(|tx| tx.tx_type().is_eip7702())
    }

    /// Returns an iterator over all blob transactions of the block
    fn blob_transactions_iter(&self) -> impl Iterator<Item = &Self::SignedTransaction> + '_ {
        self.transactions().iter().filter(|tx| tx.tx_type().is_eip4844())
    }

    /// Returns only the blob transactions, if any, from the block body.
    fn blob_transactions(&self) -> Vec<&Self::SignedTransaction> {
        self.blob_transactions_iter().collect()
    }

    /// Returns all blob versioned hashes from the block body.
    fn blob_versioned_hashes(&self) -> &[B256];

    /// Calculates a heuristic for the in-memory size of the [`BlockBody`].
    fn size(&self) -> usize;
}
