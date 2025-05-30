//! API of a signed transaction.

use crate::{
    crypto::secp256k1::recover_signer_unchecked, InMemorySize, MaybeCompact, MaybeSerde,
    MaybeSerdeBincodeCompat,
};
use alloc::{fmt, vec::Vec};
use alloy_consensus::{
    transaction::{Recovered, RlpEcdsaEncodableTx, SignerRecoverable},
    EthereumTxEnvelope, SignableTransaction,
};
use alloy_eips::eip2718::{Decodable2718, Encodable2718};
use alloy_primitives::{keccak256, Address, Signature, TxHash, B256};
use alloy_rlp::{Decodable, Encodable};
use core::hash::Hash;

pub use alloy_consensus::crypto::RecoveryError;

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullSignedTx: SignedTransaction + MaybeCompact + MaybeSerdeBincodeCompat {}
impl<T> FullSignedTx for T where T: SignedTransaction + MaybeCompact + MaybeSerdeBincodeCompat {}

/// A signed transaction.
#[auto_impl::auto_impl(&, Arc)]
pub trait SignedTransaction:
    Send
    + Sync
    + Unpin
    + Clone
    + fmt::Debug
    + PartialEq
    + Eq
    + Hash
    + Encodable
    + Decodable
    + Encodable2718
    + Decodable2718
    + alloy_consensus::Transaction
    + MaybeSerde
    + InMemorySize
    + SignerRecoverable
{
    /// Returns reference to transaction hash.
    fn tx_hash(&self) -> &TxHash;

    /// Returns whether this transaction type can be __broadcasted__ as full transaction over the
    /// network.
    ///
    /// Some transactions are not broadcastable as objects and only allowed to be broadcasted as
    /// hashes, e.g. because they missing context (e.g. blob sidecar).
    fn is_broadcastable_in_full(&self) -> bool {
        // EIP-4844 transactions are not broadcastable in full, only hashes are allowed.
        !self.is_eip4844()
    }

    /// Recover signer from signature and hash.
    ///
    /// Returns an error if the transaction's signature is invalid.
    fn try_recover(&self) -> Result<Address, RecoveryError> {
        self.recover_signer()
    }

    /// Recover signer from signature and hash _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Returns an error if the transaction's signature is invalid.
    fn try_recover_unchecked(&self) -> Result<Address, RecoveryError> {
        self.recover_signer_unchecked()
    }

    /// Same as [`SignerRecoverable::recover_signer_unchecked`] but receives a buffer to operate on.
    /// This is used during batch recovery to avoid allocating a new buffer for each
    /// transaction.
    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, RecoveryError>;

    /// Calculate transaction hash, eip2728 transaction does not contain rlp header and start with
    /// tx type.
    fn recalculate_hash(&self) -> B256 {
        keccak256(self.encoded_2718())
    }

    /// Tries to recover signer and return [`Recovered`] by cloning the type.
    #[auto_impl(keep_default_for(&, Arc))]
    fn try_clone_into_recovered(&self) -> Result<Recovered<Self>, RecoveryError> {
        self.recover_signer().map(|signer| Recovered::new_unchecked(self.clone(), signer))
    }

    /// Tries to recover signer and return [`Recovered`].
    ///
    /// Returns `Err(Self)` if the transaction's signature is invalid, see also
    /// [`SignerRecoverable::recover_signer`].
    #[auto_impl(keep_default_for(&, Arc))]
    fn try_into_recovered(self) -> Result<Recovered<Self>, Self> {
        match self.recover_signer() {
            Ok(signer) => Ok(Recovered::new_unchecked(self, signer)),
            Err(_) => Err(self),
        }
    }

    /// Consumes the type, recover signer and return [`Recovered`] _without
    /// ensuring that the signature has a low `s` value_ (EIP-2).
    ///
    /// Returns `RecoveryError` if the transaction's signature is invalid.
    #[deprecated(note = "Use try_into_recovered_unchecked instead")]
    #[auto_impl(keep_default_for(&, Arc))]
    fn into_recovered_unchecked(self) -> Result<Recovered<Self>, RecoveryError> {
        self.recover_signer_unchecked().map(|signer| Recovered::new_unchecked(self, signer))
    }

    /// Returns the [`Recovered`] transaction with the given sender.
    ///
    /// Note: assumes the given signer is the signer of this transaction.
    #[auto_impl(keep_default_for(&, Arc))]
    fn with_signer(self, signer: Address) -> Recovered<Self> {
        Recovered::new_unchecked(self, signer)
    }

    /// Returns the [`Recovered`] transaction with the given signer, using a reference to self.
    ///
    /// Note: assumes the given signer is the signer of this transaction.
    #[auto_impl(keep_default_for(&, Arc))]
    fn with_signer_ref(&self, signer: Address) -> Recovered<&Self> {
        Recovered::new_unchecked(self, signer)
    }
}

impl<T> SignedTransaction for EthereumTxEnvelope<T>
where
    T: RlpEcdsaEncodableTx + SignableTransaction<Signature> + Unpin,
    Self: Clone + PartialEq + Eq + Decodable + Decodable2718 + MaybeSerde + InMemorySize,
{
    fn tx_hash(&self) -> &TxHash {
        match self {
            Self::Legacy(tx) => tx.hash(),
            Self::Eip2930(tx) => tx.hash(),
            Self::Eip1559(tx) => tx.hash(),
            Self::Eip7702(tx) => tx.hash(),
            Self::Eip4844(tx) => tx.hash(),
        }
    }

    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        match self {
            Self::Legacy(tx) => tx.tx().encode_for_signing(buf),
            Self::Eip2930(tx) => tx.tx().encode_for_signing(buf),
            Self::Eip1559(tx) => tx.tx().encode_for_signing(buf),
            Self::Eip7702(tx) => tx.tx().encode_for_signing(buf),
            Self::Eip4844(tx) => tx.tx().encode_for_signing(buf),
        }
        let signature_hash = keccak256(buf);
        recover_signer_unchecked(self.signature(), signature_hash)
    }
}

#[cfg(feature = "op")]
mod op {
    use super::*;
    use op_alloy_consensus::{OpPooledTransaction, OpTxEnvelope};

    impl SignedTransaction for OpPooledTransaction {
        fn tx_hash(&self) -> &TxHash {
            match self {
                Self::Legacy(tx) => tx.hash(),
                Self::Eip2930(tx) => tx.hash(),
                Self::Eip1559(tx) => tx.hash(),
                Self::Eip7702(tx) => tx.hash(),
            }
        }

        fn recover_signer_unchecked_with_buf(
            &self,
            buf: &mut Vec<u8>,
        ) -> Result<Address, RecoveryError> {
            match self {
                Self::Legacy(tx) => tx.tx().encode_for_signing(buf),
                Self::Eip2930(tx) => tx.tx().encode_for_signing(buf),
                Self::Eip1559(tx) => tx.tx().encode_for_signing(buf),
                Self::Eip7702(tx) => tx.tx().encode_for_signing(buf),
            }
            let signature_hash = keccak256(buf);
            recover_signer_unchecked(self.signature(), signature_hash)
        }
    }

    impl SignedTransaction for OpTxEnvelope {
        fn tx_hash(&self) -> &TxHash {
            match self {
                Self::Legacy(tx) => tx.hash(),
                Self::Eip2930(tx) => tx.hash(),
                Self::Eip1559(tx) => tx.hash(),
                Self::Eip7702(tx) => tx.hash(),
                Self::Deposit(tx) => tx.hash_ref(),
            }
        }

        fn recover_signer_unchecked_with_buf(
            &self,
            buf: &mut Vec<u8>,
        ) -> Result<Address, RecoveryError> {
            match self {
                Self::Deposit(tx) => return Ok(tx.from),
                Self::Legacy(tx) => tx.tx().encode_for_signing(buf),
                Self::Eip2930(tx) => tx.tx().encode_for_signing(buf),
                Self::Eip1559(tx) => tx.tx().encode_for_signing(buf),
                Self::Eip7702(tx) => tx.tx().encode_for_signing(buf),
            }
            let signature_hash = keccak256(buf);
            let signature = match self {
                Self::Legacy(tx) => tx.signature(),
                Self::Eip2930(tx) => tx.signature(),
                Self::Eip1559(tx) => tx.signature(),
                Self::Eip7702(tx) => tx.signature(),
                Self::Deposit(_) => unreachable!("Deposit transactions should not be handled here"),
            };
            recover_signer_unchecked(signature, signature_hash)
        }
    }
}
