use super::TxCustom;
use alloy_consensus::{Signed, Transaction};
use alloy_eips::{eip2718::Eip2718Result, Decodable2718, Encodable2718, Typed2718};
use alloy_primitives::TxHash;
use alloy_rlp::{BufMut, Decodable, Encodable, Result as RlpResult};
use reth_codecs::Compact;
use reth_ethereum::primitives::{
    serde_bincode_compat::SerdeBincodeCompat, transaction::signed::RecoveryError, InMemorySize,
    SignedTransaction,
};
use reth_op::{
    serde_bincode_compat::transaction::OpTxEnvelope as BincodeCompatOpTransactionSigned,
    OpTransactionSigned,
};
use revm_primitives::{Address, Bytes};
use serde::{Deserialize, Serialize};

pub const TRANSFER_TX_TYPE_ID: u8 = 127;
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub struct CustomTxEnvelope<T> {
    pub inner: T,
}

pub type CustomTransaction = CustomTxEnvelope<Signed<TxCustom>>;

impl Transaction for CustomTransaction {
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

    fn input(&self) -> &revm_primitives::Bytes {
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

impl SignedTransaction for CustomTransaction {
    fn tx_hash(&self) -> &TxHash {
        self.inner.tx().hash()
    }

    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        self.inner.tx().recover_signer().map_err(|e| RecoveryError::from(e))
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        self.inner.tx().recover_signer_unchecked()
    }

    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        self.inner.tx().recover_signer_unchecked_with_buf(buf)
    }
}

impl Typed2718 for CustomTransaction {
    fn ty(&self) -> u8 {
        TRANSFER_TX_TYPE_ID
    }
}

impl Decodable2718 for CustomTransaction {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self { inner: Signed::<TxCustom>::typed_decode(ty, buf)? })
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self { inner: Signed::<TxCustom>::fallback_decode(buf)? })
    }
}

impl Encodable2718 for CustomTransaction {
    fn encode_2718_len(&self) -> usize {
        self.inner.encode_2718_len()
    }

    fn encode_2718(&self, out: &mut dyn BufMut) {
        self.inner.encode_2718(out)
    }
}

impl Decodable for CustomTransaction {
    fn decode(buf: &mut &[u8]) -> RlpResult<Self> {
        Ok(Self { inner: Signed::new(TxCustom::decode(buf)?) })
    }
}

impl Encodable for CustomTransaction {
    fn encode(&self, out: &mut dyn BufMut) {
        self.inner.tx().encode(out)
    }
}

impl InMemorySize for CustomTransaction {
    fn size(&self) -> usize {
        self.inner.tx().size()
    }
}

impl SerdeBincodeCompat for CustomTransaction {
    type BincodeRepr<'a> = BincodeCompatOpTransactionSigned<'a>;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        self.inner.as_repr()
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        Self { inner: repr.into() }
    }
}

impl Compact for CustomTransaction {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
    {
        self.inner.tx().encode(buf)
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (inner, buf) = OpTransactionSigned::from_compact(buf, len);

        (Self { inner }, buf)
    }
}
