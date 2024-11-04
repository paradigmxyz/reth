//! API of a signed transaction.

use alloc::fmt;
use core::hash::Hash;

use alloy_consensus::Transaction;
use alloy_eips::eip2718::{Decodable2718, Encodable2718};
use alloy_primitives::{keccak256, Address, Signature, TxHash, B256};

/// A signed transaction.
pub trait SignedTransaction:
    fmt::Debug
    + Clone
    + PartialEq
    + Eq
    + Hash
    + Send
    + Sync
    + serde::Serialize
    + for<'a> serde::Deserialize<'a>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + Encodable2718
    + Decodable2718
{
    /// Transaction type that is signed.
    type Transaction: Transaction;

    /// Returns reference to transaction hash.
    fn tx_hash(&self) -> &TxHash;

    /// Returns reference to transaction.
    fn transaction(&self) -> &Self::Transaction;

    /// Returns reference to signature.
    fn signature(&self) -> &Signature;

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

    /// Create a new signed transaction from a transaction and its signature.
    ///
    /// This will also calculate the transaction hash using its encoding.
    fn from_transaction_and_signature(transaction: Self::Transaction, signature: Signature)
        -> Self;

    /// Calculate transaction hash, eip2728 transaction does not contain rlp header and start with
    /// tx type.
    fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }
}
