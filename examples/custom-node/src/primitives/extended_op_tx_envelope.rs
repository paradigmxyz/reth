use op_alloy_consensus::OpTxEnvelope;
use alloy_rlp::{BufMut, Decodable, Encodable, Result as RlpResult};
use alloy_consensus::Transaction;
use alloy_primitives::{ChainId, TxHash, PrimitiveSignature};
use reth_primitives_traits::{
    serde_bincode_compat::SerdeBincodeCompat, transaction::signed::RecoveryError, InMemorySize,
    SignedTransaction,
};
use reth_codecs::Compact;
use reth_optimism_primitives::{
    serde_bincode_compat::OpTransactionSigned as BincodeCompatOpTransactionSigned,
    OpTransactionSigned, OpTxEnvelope,
};
use revm_primitives::{Address, Bytes, TxKind, U256};
use alloy_eips::{eip2930::AccessList, eip7702::SignedAuthorization};
use revm_primitives::B256;

pub enum ExtendedOpTxEnvelope<T> {
    BuiltIn(OpTxEnvelope),
    Other(OpTransactionSigned),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Hash, Eq, PartialEq)]
impl Transaction for ExtendedOpTxEnvelope {
    fn chain_id(&self) -> Option<ChainId> {
        match self {
            Self::BuiltIn(tx) => tx.chain_id(),
            Self::Other(tx) => tx.chain_id(),
        }
    }
    fn nonce(&self) -> u64 {
        match self {
            Self::BuiltIn(tx) => tx.nonce(),
            Self::Other(tx) => tx.nonce(),
        }
    }
    fn gas_limit(&self) -> u64 {
        match self {
            Self::BuiltIn(tx) => tx.gas_limit(),
            Self::Other(tx) => tx.gas_limit(),
        }
    }
    fn gas_price(&self) -> Option<u128> {
        match self {
            Self::BuiltIn(tx) => tx.gas_price(),
            Self::Other(tx) => tx.gas_price(),
        }
    }
    fn max_fee_per_gas(&self) -> u128 {
        match self {
            Self::BuiltIn(tx) => tx.max_fee_per_gas(),
            Self::Other(tx) => tx.max_fee_per_gas(),
        }
    }
    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::BuiltIn(tx) => tx.max_priority_fee_per_gas(),
            Self::Other(tx) => tx.max_priority_fee_per_gas(),
        }
    }
    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        match self {
            Self::BuiltIn(tx) => tx.max_fee_per_blob_gas(),
            Self::Other(tx) => tx.max_fee_per_blob_gas(),
        }
    }
    fn priority_fee_or_price(&self) -> u128 {
        match self {
            Self::BuiltIn(tx) => tx.priority_fee_or_price(),
            Self::Other(tx) => tx.priority_fee_or_price(),
        }
    }
    fn effective_gas_price(&self,  base_fee: Option<u64>) -> u128 {
        match self {
            Self::BuiltIn(tx) => tx.effective_gas_price(base_fee),
            Self::Other(tx) => tx.effective_gas_price(base_fee),
        }
    }
    fn is_dynamic_fee(&self) -> bool {
        match self {
            Self::BuiltIn(tx) => tx.is_dynamic_fee(),
            Self::Other(tx) => tx.is_dynamic_fee(),
        }
    }
    fn kind(&self) -> TxKind{
        match self {
            Self::BuiltIn(tx) => tx.kind(),
            Self::Other(tx) => tx.kind(),
        }
    }
    fn is_create(&self) -> bool {
        match self {
            Self::BuiltIn(tx) => tx.is_create(),
            Self::Other(tx) => false,
        }
    }
    fn value(&self) -> U256 {
        match self {
            Self::BuiltIn(tx) => tx.value(),
            Self::Other(tx) => tx.value(),
        }
    }
    fn input(&self) -> &Bytes {
        match self {
            Self::BuiltIn(tx) => tx.input(),
            Self::Other(_tx) => {
                static EMPTY_BYTES: Bytes = Bytes::new();
                &EMPTY_BYTES
            }
        }
    }
    fn access_list(&self) -> Option<&AccessList> {
        match self {
            Self::BuiltIn(tx) => tx.access_list(),
            Self::Other(tx) => tx.access_list(),
        }
    }
    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        match self {
            Self::BuiltIn(tx) => tx.blob_versioned_hashes(),
            Self::Other(tx) => tx.blob_versioned_hashes(),
        }
    }
    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        match self {
            Self::BuiltIn(tx) => tx.authorization_list(),
            Self::Other(tx) => tx.authorization_list(),
        }
    }

}

impl SignedTransaction for ExtendedOpTxEnvelope {
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        match self {
            Self::BuiltIn(tx) => tx.recover_signer(),
            Self::Other(tx) => tx.recover_signer(),
        }
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        match self {
            Self::BuiltIn(tx) => tx.recover_signer_unchecked(),
            Self::Other(tx) => tx.recover_signer_unchecked(),
        }
    }

    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        match self {
            Self::BuiltIn(tx) => tx.recover_signer_unchecked_with_buf(buf),
            Self::Other(tx) => tx.recover_signer_unchecked_with_buf(buf),
        }
    }

    fn tx_hash(&self) -> &TxHash {
        match self {
            Self::BuiltIn(tx) => tx.tx_hash(),
            Self::Other(tx) => tx.tx_hash(),
        }
    }

    fn signature(&self) -> &PrimitiveSignature {
        match self {
            Self::BuiltIn(tx) => tx.signature(),
            Self::Other(tx) => tx.signature(),
        }
    }
}


impl Typed2718 for ExtendedOpTxEnvelope {
    fn ty(&self) -> u8 {
        match self {
            Self::BuiltIn(tx) => tx.ty(),
            Self::Other(tx) => tx.transaction.tx_type() as u8,
        }
    }
}


impl Decodable2718 for ExtendedOpTxEnvelope {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        OpTxEnvelope::typed_decode(ty, buf)
            .map(Self::BuiltIn)
            .or_else(|_| OpTransactionSigned::typed_decode(ty, buf).map(Self::Other))
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        OpTxEnvelope::fallback_decode(buf)
            .map(Self::BuiltIn)
            .or_else(|_| OpTransactionSigned::fallback_decode(buf).map(Self::Other))
    }
}


impl Encodable2718 for ExtendedOpTxEnvelope {
    fn encode_2718(&self, out: &mut dyn BufMut) {
        match self {
            Self::BuiltIn(tx) => tx.encode_2718(out),
            Self::Other(tx) => tx.encode_2718(out),
        }
    }

    fn encode_2718_len(&self) -> usize {
        match self {
            Self::BuiltIn(tx) => tx.encode_2718_len(),
            Self::Other(tx) => tx.encode_2718_len(),
        }
    }
}


impl Decodable for ExtendedOpTxEnvelope {
    fn decode(buf: &mut &[u8]) -> RlpResult<Self> {
        OpTxEnvelope::decode(buf)
            .map(Self::BuiltIn)
            .or_else(|_| OpTransactionSigned::decode(buf).map(Self::Other))
    }
}

impl Encodable for ExtendedOpTxEnvelope {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::BuiltIn(tx) => tx.encode(out),
            Self::Other(tx) => tx.encode(out),
        }
    }
}

impl InMemorySize for ExtendedOpTxEnvelope {
    fn size(&self) -> usize {
        match self {
            Self::BuiltIn(tx) => tx.size(),
            Self::Other(tx) => tx.size(),
        }
    }
}

impl SerdeBincodeCompat for ExtendedOpTxEnvelope {
    type BincodeRepr<'a> = BincodeCompatOpTransactionSigned<'a>;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        match self {
            Self::BuiltIn(tx) => tx.as_repr(),
            Self::Other(tx) => tx.as_repr(),
        }
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        Self::Other(repr.into())
    }
}

impl Compact for ExtendedOpTxEnvelope {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
    {
        match self {
            Self::BuiltIn(tx) => tx.to_compact(buf),
            Self::Other(tx) => tx.to_compact(buf),
        }
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        if let Ok((builtin, rest)) = OpTxEnvelope::from_compact(buf, len) {
            (Self::BuiltIn(builtin), rest)
        } else {
            let (other, rest) = OpTransactionSigned::from_compact(buf, len);
            (Self::Other(other), rest)
        }
    }
}
