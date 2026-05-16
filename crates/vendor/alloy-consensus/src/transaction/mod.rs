//! Transaction types.

use crate::Signed;
use alloc::{borrow::Cow, vec::Vec};
use alloy_eips::{eip2930::AccessList, eip4844::DATA_GAS_PER_BLOB, eip7702::SignedAuthorization};
use alloy_primitives::{keccak256, Address, Bytes, ChainId, Selector, TxHash, TxKind, B256, U256};
use auto_impl::auto_impl;
use core::{any, fmt};

mod eip1559;
pub use eip1559::TxEip1559;

mod eip2930;
pub use eip2930::TxEip2930;

mod eip7702;
pub use eip7702::TxEip7702;

mod envelope;
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub use envelope::serde_bincode_compat as envelope_serde_bincode_compat;
pub use envelope::{EthereumTxEnvelope, TxEnvelope, TxType};

/// [EIP-4844] constants, helpers, and types.
pub mod eip4844;
pub use eip4844::{TxEip4844, TxEip4844Variant, TxEip4844WithSidecar};

mod eip4844_sidecar;
#[cfg(feature = "kzg")]
pub use eip4844_sidecar::BlobTransactionValidationError;
pub use eip4844_sidecar::TxEip4844Sidecar;

// Re-export 4844 helpers.
pub use alloy_eips::eip4844::{
    builder::{SidecarBuilder, SidecarCoder, SimpleCoder},
    utils as eip4844_utils, Blob, BlobTransactionSidecar, Bytes48,
};

pub mod pooled;
pub use pooled::PooledTransaction;

/// Re-export for convenience
pub use either::Either;

mod legacy;
pub use legacy::{from_eip155_value, to_eip155_value, TxLegacy};

mod rlp;
#[doc(hidden)]
pub use rlp::{RlpEcdsaDecodableTx, RlpEcdsaEncodableTx, RlpEcdsaTx};

mod typed;
pub use typed::{EthereumTypedTransaction, TypedTransaction};

mod tx_type;

mod meta;
pub use meta::{TransactionInfo, TransactionMeta};

mod recovered;
pub use recovered::{Recovered, SignerRecoverable};

mod hashable;
pub use hashable::TxHashable;

#[cfg(feature = "serde")]
pub use legacy::{signed_legacy_serde, untagged_legacy_serde};

/// Bincode-compatible serde implementations for transaction types.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub mod serde_bincode_compat {
    pub use super::{
        eip1559::serde_bincode_compat::*, eip2930::serde_bincode_compat::*,
        eip7702::serde_bincode_compat::*, envelope::serde_bincode_compat::*,
        legacy::serde_bincode_compat::*, typed::serde_bincode_compat::*,
    };
}

use alloy_eips::Typed2718;

/// Represents a minimal EVM transaction.
/// Currently, EIP-1559, EIP-4844, and EIP-7702 support dynamic fees.
/// We call these transactions "dynamic fee transactions".
/// We call non dynamic fee transactions(EIP-155, EIP-2930) "legacy fee transactions".
#[doc(alias = "Tx")]
#[auto_impl::auto_impl(&, Arc)]
pub trait Transaction: Typed2718 + fmt::Debug + any::Any + Send + Sync + 'static {
    /// Get `chain_id`.
    fn chain_id(&self) -> Option<ChainId>;

    /// Get `nonce`.
    fn nonce(&self) -> u64;

    /// Get `gas_limit`.
    fn gas_limit(&self) -> u64;

    /// Get `gas_price`.
    fn gas_price(&self) -> Option<u128>;

    /// For dynamic fee transactions returns the maximum fee per gas the caller is willing to pay.
    ///
    /// For legacy fee transactions this is `gas_price`.
    ///
    /// This is also commonly referred to as the "Gas Fee Cap".
    fn max_fee_per_gas(&self) -> u128;

    /// For dynamic fee transactions returns the Priority fee the caller is paying to the block
    /// author.
    ///
    /// This will return `None` for legacy fee transactions
    fn max_priority_fee_per_gas(&self) -> Option<u128>;

    /// Max fee per blob gas for EIP-4844 transaction.
    ///
    /// Returns `None` for non-eip4844 transactions.
    ///
    /// This is also commonly referred to as the "Blob Gas Fee Cap".
    fn max_fee_per_blob_gas(&self) -> Option<u128>;

    /// Return the max priority fee per gas if the transaction is a dynamic fee transaction, and
    /// otherwise return the gas price.
    ///
    /// # Warning
    ///
    /// This is different than the `max_priority_fee_per_gas` method, which returns `None` for
    /// legacy fee transactions.
    fn priority_fee_or_price(&self) -> u128;

    /// Returns the effective gas price for the given base fee.
    ///
    /// If the transaction is a legacy fee transaction, the gas price is returned.
    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128;

    /// Returns the effective tip for this transaction.
    ///
    /// For dynamic fee transactions: `min(max_fee_per_gas - base_fee, max_priority_fee_per_gas)`.
    /// For legacy fee transactions: `gas_price - base_fee`.
    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        let base_fee = base_fee as u128;

        let max_fee_per_gas = self.max_fee_per_gas();

        // Check if max_fee_per_gas is less than base_fee
        if max_fee_per_gas < base_fee {
            return None;
        }

        // Calculate the difference between max_fee_per_gas and base_fee
        let fee = max_fee_per_gas - base_fee;

        // Compare the fee with max_priority_fee_per_gas (or gas price for legacy fee transactions)
        self.max_priority_fee_per_gas()
            .map_or(Some(fee), |priority_fee| Some(fee.min(priority_fee)))
    }

    /// Returns `true` if the transaction supports dynamic fees.
    fn is_dynamic_fee(&self) -> bool;

    /// Returns the transaction kind.
    fn kind(&self) -> TxKind;

    /// Returns true if the transaction is a contract creation.
    /// We don't provide a default implementation via `kind` as it copies the 21-byte
    /// [`TxKind`] for this simple check. A proper implementation shouldn't allocate.
    fn is_create(&self) -> bool;

    /// Get the transaction's address of the contract that will be called, or the address that will
    /// receive the transfer.
    ///
    /// Returns `None` if this is a `CREATE` transaction.
    fn to(&self) -> Option<Address> {
        self.kind().into_to()
    }

    /// Get `value`.
    fn value(&self) -> U256;

    /// Get `data`.
    fn input(&self) -> &Bytes;

    /// Returns the first 4bytes of the calldata for a function call.
    ///
    /// The selector specifies the function to be called.
    fn function_selector(&self) -> Option<&Selector> {
        if self.kind().is_call() {
            self.input().get(..4).and_then(|s| TryFrom::try_from(s).ok())
        } else {
            None
        }
    }

    /// Returns the EIP-2930 `access_list` for the particular transaction type. Returns `None` for
    /// older transaction types.
    fn access_list(&self) -> Option<&AccessList>;

    /// Blob versioned hashes for eip4844 transaction. For previous transaction types this is
    /// `None`.
    fn blob_versioned_hashes(&self) -> Option<&[B256]>;

    /// Returns the number of blobs of this transaction.
    ///
    /// This is convenience function for `len(blob_versioned_hashes)`.
    ///
    /// Returns `None` for non-eip4844 transactions.
    fn blob_count(&self) -> Option<u64> {
        self.blob_versioned_hashes().map(|h| h.len() as u64)
    }

    /// Returns the total gas for all blobs in this transaction.
    ///
    /// Returns `None` for non-eip4844 transactions.
    #[inline]
    fn blob_gas_used(&self) -> Option<u64> {
        // SAFETY: we don't expect u64::MAX / DATA_GAS_PER_BLOB hashes in a single transaction
        self.blob_count().map(|blobs| blobs * DATA_GAS_PER_BLOB)
    }

    /// Returns the [`SignedAuthorization`] list of the transaction.
    ///
    /// Returns `None` if this transaction is not EIP-7702.
    fn authorization_list(&self) -> Option<&[SignedAuthorization]>;

    /// Returns the number of [`SignedAuthorization`]s in this transactions
    ///
    /// This is convenience function for `len(authorization_list)`.
    ///
    /// Returns `None` for non-eip7702 transactions.
    fn authorization_count(&self) -> Option<u64> {
        self.authorization_list().map(|auths| auths.len() as u64)
    }
}

/// A typed transaction envelope.
pub trait TransactionEnvelope: Transaction {
    /// The enum of transaction types.
    type TxType: Typed2718;

    /// Returns the transaction type.
    fn tx_type(&self) -> Self::TxType;
}

/// A signable transaction.
///
/// A transaction can have multiple signature types. This is usually
/// [`alloy_primitives::Signature`], however, it may be different for future EIP-2718
/// transaction types, or in other networks. For example, in Optimism, the deposit transaction
/// signature is the unit type `()`.
#[doc(alias = "SignableTx", alias = "TxSignable")]
pub trait SignableTransaction<Signature>: Transaction {
    /// Sets `chain_id`.
    ///
    /// Prefer [`set_chain_id_checked`](Self::set_chain_id_checked).
    fn set_chain_id(&mut self, chain_id: ChainId);

    /// Set `chain_id` if it is not already set. Checks that the provided `chain_id` matches the
    /// existing `chain_id` if it is already set, returning `false` if they do not match.
    fn set_chain_id_checked(&mut self, chain_id: ChainId) -> bool {
        match self.chain_id() {
            Some(tx_chain_id) => {
                if tx_chain_id != chain_id {
                    return false;
                }
                // Chain ID already matches, no need to set it again
            }
            None => {
                self.set_chain_id(chain_id);
            }
        }
        true
    }

    /// RLP-encodes the transaction for signing.
    fn encode_for_signing(&self, out: &mut dyn alloy_rlp::BufMut);

    /// Returns the length of the RLP-encoded transaction for signing.
    fn payload_len_for_signature(&self) -> usize;

    /// RLP-encodes the transaction for signing it. Used to calculate `signature_hash`.
    ///
    /// See [`SignableTransaction::encode_for_signing`].
    fn encoded_for_signing(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.payload_len_for_signature());
        self.encode_for_signing(&mut buf);
        buf
    }

    /// Calculate the signing hash for the transaction.
    fn signature_hash(&self) -> B256 {
        keccak256(self.encoded_for_signing())
    }

    /// Convert to a [`Signed`] object.
    fn into_signed(self, signature: Signature) -> Signed<Self, Signature>
    where
        Self: Sized,
    {
        Signed::new_unhashed(self, signature)
    }
}

#[cfg(feature = "serde")]
impl<T: Transaction> Transaction for alloy_serde::WithOtherFields<T> {
    #[inline]
    fn chain_id(&self) -> Option<ChainId> {
        self.inner.chain_id()
    }

    #[inline]
    fn nonce(&self) -> u64 {
        self.inner.nonce()
    }

    #[inline]
    fn gas_limit(&self) -> u64 {
        self.inner.gas_limit()
    }

    #[inline]
    fn gas_price(&self) -> Option<u128> {
        self.inner.gas_price()
    }

    #[inline]
    fn max_fee_per_gas(&self) -> u128 {
        self.inner.max_fee_per_gas()
    }

    #[inline]
    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.inner.max_priority_fee_per_gas()
    }

    #[inline]
    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.inner.max_fee_per_blob_gas()
    }

    #[inline]
    fn priority_fee_or_price(&self) -> u128 {
        self.inner.priority_fee_or_price()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.inner.effective_gas_price(base_fee)
    }

    #[inline]
    fn is_dynamic_fee(&self) -> bool {
        self.inner.is_dynamic_fee()
    }

    #[inline]
    fn kind(&self) -> TxKind {
        self.inner.kind()
    }

    #[inline]
    fn is_create(&self) -> bool {
        self.inner.is_create()
    }

    #[inline]
    fn value(&self) -> U256 {
        self.inner.value()
    }

    #[inline]
    fn input(&self) -> &Bytes {
        self.inner.input()
    }

    #[inline]
    fn access_list(&self) -> Option<&AccessList> {
        self.inner.access_list()
    }

    #[inline]
    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        self.inner.blob_versioned_hashes()
    }

    #[inline]
    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        self.inner.authorization_list()
    }
}

impl<L, R> Transaction for either::Either<L, R>
where
    L: Transaction,
    R: Transaction,
{
    fn chain_id(&self) -> Option<ChainId> {
        match self {
            Self::Left(tx) => tx.chain_id(),
            Self::Right(tx) => tx.chain_id(),
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            Self::Left(tx) => tx.nonce(),
            Self::Right(tx) => tx.nonce(),
        }
    }

    fn gas_limit(&self) -> u64 {
        match self {
            Self::Left(tx) => tx.gas_limit(),
            Self::Right(tx) => tx.gas_limit(),
        }
    }

    fn gas_price(&self) -> Option<u128> {
        match self {
            Self::Left(tx) => tx.gas_price(),
            Self::Right(tx) => tx.gas_price(),
        }
    }

    fn max_fee_per_gas(&self) -> u128 {
        match self {
            Self::Left(tx) => tx.max_fee_per_gas(),
            Self::Right(tx) => tx.max_fee_per_gas(),
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::Left(tx) => tx.max_priority_fee_per_gas(),
            Self::Right(tx) => tx.max_priority_fee_per_gas(),
        }
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        match self {
            Self::Left(tx) => tx.max_fee_per_blob_gas(),
            Self::Right(tx) => tx.max_fee_per_blob_gas(),
        }
    }

    fn priority_fee_or_price(&self) -> u128 {
        match self {
            Self::Left(tx) => tx.priority_fee_or_price(),
            Self::Right(tx) => tx.priority_fee_or_price(),
        }
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        match self {
            Self::Left(tx) => tx.effective_gas_price(base_fee),
            Self::Right(tx) => tx.effective_gas_price(base_fee),
        }
    }

    fn effective_tip_per_gas(&self, base_fee: u64) -> Option<u128> {
        match self {
            Self::Left(tx) => tx.effective_tip_per_gas(base_fee),
            Self::Right(tx) => tx.effective_tip_per_gas(base_fee),
        }
    }

    fn is_dynamic_fee(&self) -> bool {
        match self {
            Self::Left(tx) => tx.is_dynamic_fee(),
            Self::Right(tx) => tx.is_dynamic_fee(),
        }
    }

    fn kind(&self) -> TxKind {
        match self {
            Self::Left(tx) => tx.kind(),
            Self::Right(tx) => tx.kind(),
        }
    }

    fn is_create(&self) -> bool {
        match self {
            Self::Left(tx) => tx.is_create(),
            Self::Right(tx) => tx.is_create(),
        }
    }

    fn to(&self) -> Option<Address> {
        match self {
            Self::Left(tx) => tx.to(),
            Self::Right(tx) => tx.to(),
        }
    }

    fn value(&self) -> U256 {
        match self {
            Self::Left(tx) => tx.value(),
            Self::Right(tx) => tx.value(),
        }
    }

    fn input(&self) -> &Bytes {
        match self {
            Self::Left(tx) => tx.input(),
            Self::Right(tx) => tx.input(),
        }
    }

    fn function_selector(&self) -> Option<&Selector> {
        match self {
            Self::Left(tx) => tx.function_selector(),
            Self::Right(tx) => tx.function_selector(),
        }
    }

    fn access_list(&self) -> Option<&AccessList> {
        match self {
            Self::Left(tx) => tx.access_list(),
            Self::Right(tx) => tx.access_list(),
        }
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        match self {
            Self::Left(tx) => tx.blob_versioned_hashes(),
            Self::Right(tx) => tx.blob_versioned_hashes(),
        }
    }

    fn blob_count(&self) -> Option<u64> {
        match self {
            Self::Left(tx) => tx.blob_count(),
            Self::Right(tx) => tx.blob_count(),
        }
    }

    fn blob_gas_used(&self) -> Option<u64> {
        match self {
            Self::Left(tx) => tx.blob_gas_used(),
            Self::Right(tx) => tx.blob_gas_used(),
        }
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        match self {
            Self::Left(tx) => tx.authorization_list(),
            Self::Right(tx) => tx.authorization_list(),
        }
    }

    fn authorization_count(&self) -> Option<u64> {
        match self {
            Self::Left(tx) => tx.authorization_count(),
            Self::Right(tx) => tx.authorization_count(),
        }
    }
}

/// Trait for types that provide access to a transaction hash reference.
///
/// This trait is implemented by types that contain or can provide a reference to a
/// transaction hash ([`TxHash`]). It provides a standard interface for accessing
/// transaction hashes without requiring ownership.
#[auto_impl(&, &mut, Box)]
pub trait TxHashRef {
    /// Returns a reference to the transaction hash.
    ///
    /// This assumes the implementing type already owns or has computed the transaction hash.
    fn tx_hash(&self) -> &TxHash;
}

impl<T: TxHashRef> TxHashRef for Recovered<T> {
    fn tx_hash(&self) -> &TxHash {
        self.inner().tx_hash()
    }
}

impl<T: TxHashRef> TxHashRef for alloy_eips::eip2718::WithEncoded<T> {
    fn tx_hash(&self) -> &TxHash {
        self.value().tx_hash()
    }
}

impl<T: TxHashRef + Clone> TxHashRef for Cow<'_, T> {
    fn tx_hash(&self) -> &TxHash {
        (**self).tx_hash()
    }
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use crate::{Signed, TransactionEnvelope, TxEip1559, TxEnvelope, TxType};
    use alloy_primitives::Signature;

    #[test]
    fn test_custom_envelope() {
        use serde::{Serialize, Serializer};
        fn serialize_with<S: Serializer>(
            tx: &Signed<TxEip1559>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            #[derive(Serialize)]
            struct WithExtra<'a> {
                #[serde(flatten)]
                inner: &'a Signed<TxEip1559>,
                extra: &'a str,
            }
            WithExtra { inner: tx, extra: "extra" }.serialize(serializer)
        }

        #[derive(Debug, Clone, TransactionEnvelope)]
        #[envelope(alloy_consensus = crate, tx_type_name = MyTxType)]
        enum MyEnvelope {
            #[envelope(flatten)]
            Ethereum(TxEnvelope),
            #[envelope(ty = 10)]
            MyTx(Signed<TxEip1559>),
            #[envelope(ty = 11)]
            #[serde(serialize_with = "serialize_with")]
            AnotherMyTx(Signed<TxEip1559>),
        }

        assert_eq!(u8::from(MyTxType::Ethereum(TxType::Eip1559)), 2);
        assert_eq!(u8::from(MyTxType::MyTx), 10);
        assert_eq!(MyTxType::try_from(2u8).unwrap(), MyTxType::Ethereum(TxType::Eip1559));
        assert_eq!(MyTxType::try_from(10u8).unwrap(), MyTxType::MyTx);

        let tx = Signed::new_unhashed(
            TxEip1559 {
                chain_id: 1,
                gas_limit: 21000,
                max_fee_per_gas: 1000,
                max_priority_fee_per_gas: 1000,
                ..Default::default()
            },
            Signature::new(Default::default(), Default::default(), Default::default()),
        );

        let my_tx = serde_json::to_string(&MyEnvelope::MyTx(tx.clone())).unwrap();
        let another_my_tx = serde_json::to_string(&MyEnvelope::AnotherMyTx(tx)).unwrap();

        assert_eq!(
            my_tx,
            r#"{"type":"0xa","chainId":"0x1","nonce":"0x0","gas":"0x5208","maxFeePerGas":"0x3e8","maxPriorityFeePerGas":"0x3e8","to":null,"value":"0x0","accessList":[],"input":"0x","r":"0x0","s":"0x0","yParity":"0x0","v":"0x0","hash":"0x2eaaca5609601faae806f5147abb8f51ae91cba12604bedc23a16f2776d5a97b"}"#
        );
        assert_eq!(
            another_my_tx,
            r#"{"type":"0xb","chainId":"0x1","nonce":"0x0","gas":"0x5208","maxFeePerGas":"0x3e8","maxPriorityFeePerGas":"0x3e8","to":null,"value":"0x0","accessList":[],"input":"0x","r":"0x0","s":"0x0","yParity":"0x0","v":"0x0","hash":"0x2eaaca5609601faae806f5147abb8f51ae91cba12604bedc23a16f2776d5a97b","extra":"extra"}"#
        );
    }
}
