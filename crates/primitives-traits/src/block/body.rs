//! Block body abstraction.

use alloc::{fmt, vec::Vec};
use alloy_consensus::Transaction;
use alloy_eips::{eip4844::DATA_GAS_PER_BLOB, eip4895::Withdrawals};

use crate::{
    FullSignedTx, InMemorySize, MaybeArbitrary, MaybeSerde, MaybeSerdeBincodeCompat,
    SignedTransaction,
};

/// Helper trait that unifies all behaviour required by transaction to support full node operations.
pub trait FullBlockBody: BlockBody<Transaction: FullSignedTx> {}

impl<T> FullBlockBody for T where T: BlockBody<Transaction: FullSignedTx> {}

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
    + MaybeArbitrary
    + MaybeSerdeBincodeCompat
    + 'static
{
    /// Ordered list of signed transactions as committed in block.
    type Transaction: SignedTransaction;

    /// Ommer header type.
    type OmmerHeader;

    /// Returns reference to transactions in block.
    fn transactions(&self) -> &[Self::Transaction];

    /// Consume the block body and return a [`Vec`] of transactions.
    fn into_transactions(self) -> Vec<Self::Transaction>;

    /// Returns block withdrawals if any.
    fn withdrawals(&self) -> Option<&Withdrawals>;

    /// Returns block ommers if any.
    fn ommers(&self) -> Option<&[Self::OmmerHeader]>;

    /// Calculates the total blob gas used by _all_ EIP-4844 transactions in the block.
    fn blob_gas_used(&self) -> u64 {
        // TODO(mattss): simplify after <https://github.com/alloy-rs/alloy/pull/1704>
        self.transactions()
            .iter()
            .filter_map(|tx| tx.blob_versioned_hashes())
            .map(|hashes| hashes.len() as u64 * DATA_GAS_PER_BLOB)
            .sum()
    }
}
