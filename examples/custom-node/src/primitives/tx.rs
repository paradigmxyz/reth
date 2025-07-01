use super::TxPayment;
use alloy_consensus::{
    crypto::{
        secp256k1::{recover_signer, recover_signer_unchecked},
        RecoveryError,
    },
    transaction::SignerRecoverable,
    Signed, Transaction, TransactionEnvelope,
};
use alloy_eips::{
    eip2718::{Eip2718Result, IsTyped2718},
    Decodable2718, Encodable2718, Typed2718,
};
use alloy_primitives::{bytes::Buf, Sealed, Signature, TxHash, B256};
use alloy_rlp::{BufMut, Decodable, Encodable, Result as RlpResult};
use op_alloy_consensus::{OpTxEnvelope, TxDeposit};
use reth_codecs::{
    alloy::transaction::{FromTxCompact, ToTxCompact},
    Compact,
};
use reth_ethereum::primitives::{serde_bincode_compat::RlpBincode, InMemorySize};
use reth_op::{primitives::SignedTransaction, OpTransaction};
use revm_primitives::{Address, Bytes};
use serde::{Deserialize, Serialize};

/// Either [`OpTxEnvelope`] or [`CustomTransactionEnvelope`].
#[derive(Debug, Clone, TransactionEnvelope)]
#[envelope(tx_type_name = TxTypeCustom)]
pub enum CustomTransaction {
    /// A regular Optimism transaction as defined by [`OpTxEnvelope`].
    #[envelope(flatten)]
    Op(OpTxEnvelope),
    /// A [`TxPayment`] tagged with type 0x7E.
    #[envelope(ty = 42)]
    Payment(CustomTransactionEnvelope),
}

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

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
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

impl RlpBincode for CustomTransactionEnvelope {}
impl RlpBincode for CustomTransaction {}

impl reth_codecs::alloy::transaction::Envelope for CustomTransactionEnvelope {
    fn signature(&self) -> &Signature {
        self.inner.signature()
    }

    fn tx_type(&self) -> Self::TxType {
        TxTypeCustom::Payment
    }
}

impl Compact for CustomTransactionEnvelope {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
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

impl reth_codecs::Compact for CustomTransaction {
    fn to_compact<Buf>(&self, buf: &mut Buf) -> usize
    where
        Buf: BufMut + AsMut<[u8]>,
    {
        buf.put_u8(self.ty());
        match self {
            Self::Op(tx) => tx.to_compact(buf),
            Self::Payment(tx) => tx.to_compact(buf),
        }
    }

    fn from_compact(mut buf: &[u8], len: usize) -> (Self, &[u8]) {
        let type_byte = buf.get_u8();

        if <OpTxEnvelope as IsTyped2718>::is_type(type_byte) {
            let (tx, remaining) = OpTxEnvelope::from_compact(buf, len);
            return (Self::Op(tx), remaining);
        }

        let (tx, remaining) = CustomTransactionEnvelope::from_compact(buf, len);
        (Self::Payment(tx), remaining)
    }
}

impl OpTransaction for CustomTransactionEnvelope {
    fn is_deposit(&self) -> bool {
        false
    }

    fn as_deposit(&self) -> Option<&Sealed<TxDeposit>> {
        None
    }
}

impl OpTransaction for CustomTransaction {
    fn is_deposit(&self) -> bool {
        match self {
            CustomTransaction::Op(op) => op.is_deposit(),
            CustomTransaction::Payment(payment) => payment.is_deposit(),
        }
    }

    fn as_deposit(&self) -> Option<&Sealed<TxDeposit>> {
        match self {
            CustomTransaction::Op(op) => op.as_deposit(),
            CustomTransaction::Payment(payment) => payment.as_deposit(),
        }
    }
}

impl SignerRecoverable for CustomTransaction {
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        match self {
            CustomTransaction::Op(tx) => SignerRecoverable::recover_signer(tx),
            CustomTransaction::Payment(tx) => SignerRecoverable::recover_signer(tx),
        }
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        match self {
            CustomTransaction::Op(tx) => SignerRecoverable::recover_signer_unchecked(tx),
            CustomTransaction::Payment(tx) => SignerRecoverable::recover_signer_unchecked(tx),
        }
    }
}

impl SignedTransaction for CustomTransaction {
    fn tx_hash(&self) -> &B256 {
        match self {
            CustomTransaction::Op(tx) => SignedTransaction::tx_hash(tx),
            CustomTransaction::Payment(tx) => SignedTransaction::tx_hash(tx),
        }
    }
}

impl InMemorySize for CustomTransaction {
    fn size(&self) -> usize {
        match self {
            CustomTransaction::Op(tx) => InMemorySize::size(tx),
            CustomTransaction::Payment(tx) => InMemorySize::size(tx),
        }
    }
}
