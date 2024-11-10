//! API of a signed transaction.

use alloc::fmt;
use core::hash::Hash;

use alloy_eips::{
    eip2718::{Decodable2718, Encodable2718},
    eip2930::AccessList,
    eip7702::SignedAuthorization,
};
use alloy_primitives::{
    keccak256, Address, Bytes, ChainId, PrimitiveSignature as Signature, TxHash, TxKind, B256, U256,
};
use reth_codecs::Compact;
use revm_primitives::TxEnv;

use crate::{FullTransaction, Transaction};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullSignedTx: SignedTransaction<Transaction: FullTransaction> + Compact {}

impl<T> FullSignedTx for T where T: SignedTransaction<Transaction: FullTransaction> + Compact {}

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
    + Transaction
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

    /// Fills [`TxEnv`] with an [`Address`] and transaction.
    fn fill_tx_env(&self, tx_env: &mut TxEnv, sender: Address);
}

impl<T: SignedTransaction> alloy_consensus::Transaction for T {
    fn chain_id(&self) -> Option<ChainId> {
        self.transaction().chain_id()
    }

    fn nonce(&self) -> u64 {
        self.transaction().nonce()
    }

    fn gas_limit(&self) -> u64 {
        self.transaction().gas_limit()
    }

    fn gas_price(&self) -> Option<u128> {
        self.transaction().gas_price()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.transaction().max_fee_per_gas()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.transaction().max_priority_fee_per_gas()
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.transaction().max_fee_per_blob_gas()
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.transaction().priority_fee_or_price()
    }

    fn kind(&self) -> TxKind {
        self.transaction().kind()
    }

    fn value(&self) -> U256 {
        self.transaction().value()
    }

    fn input(&self) -> &Bytes {
        self.transaction().input()
    }

    fn ty(&self) -> u8 {
        self.transaction().ty()
    }

    fn access_list(&self) -> Option<&AccessList> {
        self.transaction().access_list()
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.transaction().blob_versioned_hashes()
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.transaction().authorization_list()
    }
}

impl<T: SignedTransaction> Transaction for T {
    fn signature_hash(&self) -> B256 {
        self.transaction().signature_hash()
    }

    fn kind(&self) -> TxKind {
        self.transaction().kind()
    }

    fn is_dynamic_fee(&self) -> bool {
        self.transaction().is_dynamic_fee()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.transaction().effective_gas_price(base_fee)
    }

    fn encode_without_signature(&self, out: &mut dyn bytes::BufMut) {
        self.transaction().encode_without_signature(out)
    }

    fn size(&self) -> usize {
        self.transaction().size()
    }
}
