use super::{TxPayment, TxTypeCustom};
use alloy_consensus::{
    crypto::{
        secp256k1::{recover_signer, recover_signer_unchecked},
        RecoveryError,
    },
    transaction::SignerRecoverable,
    SignableTransaction, Signed, Transaction,
};
use alloy_eips::{eip2718::Eip2718Result, Decodable2718, Encodable2718, Typed2718};
use alloy_primitives::{keccak256, Signature, TxHash};
use alloy_rlp::{BufMut, Decodable, Encodable, Result as RlpResult};
use op_alloy_consensus::OpTxEnvelope;
use reth_codecs::{
    alloy::transaction::{FromTxCompact, ToTxCompact},
    Compact,
};
use reth_ethereum::primitives::{serde_bincode_compat::SerdeBincodeCompat, InMemorySize};
use reth_op::{
    primitives::{Extended, SignedTransaction},
    OpTransaction,
};
use revm_primitives::{Address, Bytes};
use serde::{Deserialize, Serialize};

/// An [`OpTxEnvelope`] that is [`Extended`] by one more variant of [`CustomTransactionEnvelope`].
pub type CustomTransaction = ExtendedOpTxEnvelope<CustomTransactionEnvelope>;

/// A [`SignedTransaction`] implementation that combines the [`OpTxEnvelope`] and another
/// transaction type.
pub type ExtendedOpTxEnvelope<T> = Extended<OpTxEnvelope, T>;

#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct CustomTransactionEnvelope {
    pub inner: Signed<TxPayment>,
}

impl Transaction for CustomTransactionEnvelope {
    fn chain_id(&self) -> Option<alloy_primitives::ChainId> {
        self.inner.tx().chain_id()
    }

    fn nonce(&self) -> u64 {
        self.inner.tx().nonce()
    }

    fn gas_limit(&self) -> u64 {
        self.inner.tx().gas_limit()
    }

    fn gas_price(&self) -> Option<u128> {
        self.inner.tx().gas_price()
    }

    fn max_fee_per_gas(&self) -> u128 {
        self.inner.tx().max_fee_per_gas()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.inner.tx().max_priority_fee_per_gas()
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        self.inner.tx().max_fee_per_blob_gas()
    }

    fn priority_fee_or_price(&self) -> u128 {
        self.inner.tx().priority_fee_or_price()
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        self.inner.tx().effective_gas_price(base_fee)
    }

    fn is_dynamic_fee(&self) -> bool {
        self.inner.tx().is_dynamic_fee()
    }

    fn kind(&self) -> revm_primitives::TxKind {
        self.inner.tx().kind()
    }

    fn is_create(&self) -> bool {
        false
    }

    fn value(&self) -> revm_primitives::U256 {
        self.inner.tx().value()
    }

    fn input(&self) -> &Bytes {
        // CustomTransactions have no input data
        static EMPTY_BYTES: Bytes = Bytes::new();
        &EMPTY_BYTES
    }

    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        self.inner.tx().access_list()
    }

    fn blob_versioned_hashes(&self) -> Option<&[revm_primitives::B256]> {
        self.inner.tx().blob_versioned_hashes()
    }

    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        self.inner.tx().authorization_list()
    }
}

impl SignerRecoverable for CustomTransactionEnvelope {
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        let signature_hash = self.inner.signature_hash();
        recover_signer(self.inner.signature(), signature_hash)
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        let signature_hash = self.inner.signature_hash();
        recover_signer_unchecked(self.inner.signature(), signature_hash)
    }
}

impl SignedTransaction for CustomTransactionEnvelope {
    fn tx_hash(&self) -> &TxHash {
        self.inner.hash()
    }

    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        self.inner.tx().encode_for_signing(buf);
        let signature_hash = keccak256(buf);
        recover_signer_unchecked(self.inner.signature(), signature_hash)
    }
}

impl Typed2718 for CustomTransactionEnvelope {
    fn ty(&self) -> u8 {
        self.inner.tx().ty()
    }
}

impl Decodable2718 for CustomTransactionEnvelope {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self { inner: Signed::<TxPayment>::typed_decode(ty, buf)? })
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self { inner: Signed::<TxPayment>::fallback_decode(buf)? })
    }
}

impl Encodable2718 for CustomTransactionEnvelope {
    fn encode_2718_len(&self) -> usize {
        self.inner.encode_2718_len()
    }

    fn encode_2718(&self, out: &mut dyn BufMut) {
        self.inner.encode_2718(out)
    }
}

impl Decodable for CustomTransactionEnvelope {
    fn decode(buf: &mut &[u8]) -> RlpResult<Self> {
        let inner = Signed::<TxPayment>::decode_2718(buf)?;
        Ok(CustomTransactionEnvelope { inner })
    }
}

impl Encodable for CustomTransactionEnvelope {
    fn encode(&self, out: &mut dyn BufMut) {
        self.inner.tx().encode(out)
    }
}

impl InMemorySize for CustomTransactionEnvelope {
    fn size(&self) -> usize {
        self.inner.tx().size()
    }
}

impl FromTxCompact for CustomTransactionEnvelope {
    type TxType = TxTypeCustom;

    fn from_tx_compact(buf: &[u8], _tx_type: Self::TxType, signature: Signature) -> (Self, &[u8])
    where
        Self: Sized,
    {
        let (tx, buf) = TxPayment::from_compact(buf, buf.len());
        let tx = Signed::new_unhashed(tx, signature);
        (CustomTransactionEnvelope { inner: tx }, buf)
    }
}

impl ToTxCompact for CustomTransactionEnvelope {
    fn to_tx_compact(&self, buf: &mut (impl BufMut + AsMut<[u8]>)) {
        self.inner.tx().to_compact(buf);
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BincodeCompatSignedTxCustom(pub Signed<TxPayment>);

impl SerdeBincodeCompat for CustomTransactionEnvelope {
    type BincodeRepr<'a> = BincodeCompatSignedTxCustom;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        BincodeCompatSignedTxCustom(self.inner.clone())
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        Self { inner: repr.0.clone() }
    }
}

impl reth_codecs::alloy::transaction::Envelope for CustomTransactionEnvelope {
    fn signature(&self) -> &Signature {
        self.inner.signature()
    }

    fn tx_type(&self) -> Self::TxType {
        TxTypeCustom::Custom
    }
}

impl Compact for CustomTransactionEnvelope {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
    {
        self.inner.tx().to_compact(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (signature, rest) = Signature::from_compact(buf, len);
        let (inner, buf) = <TxPayment as Compact>::from_compact(rest, len);
        let signed = Signed::new_unhashed(inner, signature);
        (CustomTransactionEnvelope { inner: signed }, buf)
    }
}

impl OpTransaction for CustomTransactionEnvelope {
    fn is_deposit(&self) -> bool {
        false
    }
}
