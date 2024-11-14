//! API of a signed transaction.

use alloc::fmt;
use core::hash::Hash;
#[cfg(feature = "std")]
use std::sync::LazyLock;

use alloy_eips::eip2718::{Decodable2718, Encodable2718};
use alloy_primitives::{keccak256, Address, PrimitiveSignature, TxHash, B256};
#[cfg(not(feature = "std"))]
use once_cell::sync::Lazy as LazyLock;
use rayon::prelude::{IntoParallelIterator, ParallelIterator};
use reth_codecs::Compact;
use revm_primitives::TxEnv;

use crate::{transaction::TransactionExt, FullTransaction, MaybeArbitrary, Transaction};

/// Expected number of transactions where we can expect a speed-up by recovering the senders in
/// parallel.
pub static PARALLEL_SENDER_RECOVERY_THRESHOLD: LazyLock<usize> =
    LazyLock::new(|| match rayon::current_num_threads() {
        0..=1 => usize::MAX,
        2..=8 => 10,
        _ => 5,
    });

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullSignedTx: SignedTransaction<Transaction: FullTransaction> + Compact {}

impl<T> FullSignedTx for T where T: SignedTransaction<Transaction: FullTransaction> + Compact {}

/// A signed transaction.
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
    + serde::Serialize
    + for<'a> serde::Deserialize<'a>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + Encodable2718
    + Decodable2718
    + TransactionExt
    + MaybeArbitrary
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

    /// Recovers a list of signers from a transaction list iterator.
    ///
    /// Returns `None`, if some transaction's signature is invalid, see also
    /// [`Self::recover_signer`].
    fn recover_signers<'a, T>(txes: T, num_txes: usize) -> Option<Vec<Address>>
    where
        T: IntoParallelIterator<Item = &'a Self> + IntoIterator<Item = &'a Self> + Send,
    {
        if num_txes < *PARALLEL_SENDER_RECOVERY_THRESHOLD {
            txes.into_iter().map(|tx| tx.recover_signer()).collect()
        } else {
            txes.into_par_iter().map(|tx| tx.recover_signer()).collect()
        }
    }

    /// Create a new signed transaction from a transaction and its signature.
    ///
    /// This will also calculate the transaction hash using its encoding.
    fn from_transaction_and_signature(
        transaction: Self::Transaction,
        signature: PrimitiveSignature,
    ) -> Self;

    /// Calculate transaction hash, eip2728 transaction does not contain rlp header and start with
    /// tx type.
    fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }

    /// Fills [`TxEnv`] with an [`Address`] and transaction.
    fn fill_tx_env(&self, tx_env: &mut TxEnv, sender: Address);
}

impl<T: SignedTransaction> TransactionExt for T {
    type Type = <T::Transaction as TransactionExt>::Type;

    fn signature_hash(&self) -> B256 {
        self.transaction().signature_hash()
    }
}
