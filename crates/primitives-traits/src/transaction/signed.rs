//! API of a signed transaction.

use alloc::fmt;
use core::hash::Hash;

use alloy_eips::eip2718::{Decodable2718, Encodable2718};
use alloy_primitives::{keccak256, Address, PrimitiveSignature, TxHash, B256};
use reth_codecs::Compact;

use crate::{FillTxEnv, FullTransaction, InMemorySize, MaybeArbitrary, MaybeSerde, Transaction};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullSignedTx:
    SignedTransaction<Transaction: FullTransaction> + FillTxEnv + Compact
{
}

impl<T> FullSignedTx for T where
    T: SignedTransaction<Transaction: FullTransaction> + FillTxEnv + Compact
{
}

/// A signed transaction.
#[auto_impl::auto_impl(&, Arc)]
pub trait SignedTransaction:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + Hash
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + Encodable2718
    + Decodable2718
    + alloy_consensus::Transaction
    + MaybeSerde
    + MaybeArbitrary
    + InMemorySize
{
    /// Transaction type that is signed.
    type Transaction: Transaction;

    /// Returns reference to transaction hash.
    fn tx_hash(&self) -> &TxHash;

    /// Returns reference to transaction.
    fn transaction(&self) -> &Self::Transaction;

    /// Returns reference to signature.
    fn signature(&self) -> &PrimitiveSignature;

    /// Recover signer from signature and hash.
    ///
    /// Returns `None` if the transaction's signature is invalid following [EIP-2](https://eips.ethereum.org/EIPS/eip-2), see also `reth_primitives::transaction::recover_signer`.
    ///
    /// Note:
    ///
    /// This can fail for some early ethereum mainnet transactions pre EIP-2, use
    /// [`Self::recover_signer_unchecked`] if you want to recover the signer without ensuring that
    /// the signature has a low `s` value.
    fn recover_signer(&self) -> Option<Address>;

    /// Recover signer from signature and hash _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Returns `None` if the transaction's signature is invalid, see also
    /// `reth_primitives::transaction::recover_signer_unchecked`.
    fn recover_signer_unchecked(&self) -> Option<Address>;

    /// Calculate transaction hash, eip2728 transaction does not contain rlp header and start with
    /// tx type.
    fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }
}

/// Helper trait used in testing.
#[cfg(feature = "test-utils")]
pub trait SignedTransactionTesting: SignedTransaction {
    /// Create a new signed transaction from a transaction and its signature.
    ///
    /// This will also calculate the transaction hash using its encoding.
    fn from_transaction_and_signature(
        transaction: Self::Transaction,
        signature: PrimitiveSignature,
    ) -> Self;
}
