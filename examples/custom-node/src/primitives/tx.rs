use super::{TxPayment, TRANSFER_TX_TYPE_ID};
use alloy_consensus::{
    crypto::{
        secp256k1::{recover_signer, recover_signer_unchecked},
        RecoveryError,
    },
    transaction::SignerRecoverable,
    SignableTransaction, Signed, Transaction,
};
use alloy_eips::{eip2718::Eip2718Result, Decodable2718, Encodable2718, Typed2718};
use alloy_primitives::{keccak256, Sealed, Signature, TxHash};
use alloy_rlp::{BufMut, Decodable, Encodable, Result as RlpResult};
use op_alloy_consensus::{OpTxEnvelope, TxDeposit};
use reth_codecs::{
    alloy::transaction::{FromTxCompact, ToTxCompact},
    Compact,
};
use reth_ethereum::primitives::{serde_bincode_compat::SerdeBincodeCompat, InMemorySize};
use reth_op::{primitives::SignedTransaction, OpTransaction};
use revm_primitives::{Address, Bytes};
use serde::{Deserialize, Serialize};

/// Custom transaction envelope that extends OpTxEnvelope with our custom transaction type
#[derive(Debug, Clone, Serialize, Deserialize, Hash, Eq, PartialEq)]
pub enum CustomTransaction {
    /// Ethereum transactions (includes all standard transaction types)
    Ethereum(OpTxEnvelope),
    /// Custom payment transaction
    Custom(Signed<TxPayment>),
}

// Implement Transaction trait
impl Transaction for CustomTransaction {
    fn chain_id(&self) -> Option<alloy_primitives::ChainId> {
        match self {
            Self::Ethereum(tx) => tx.chain_id(),
            Self::Custom(tx) => tx.tx().chain_id(),
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            Self::Ethereum(tx) => tx.nonce(),
            Self::Custom(tx) => tx.tx().nonce(),
        }
    }

    fn gas_limit(&self) -> u64 {
        match self {
            Self::Ethereum(tx) => tx.gas_limit(),
            Self::Custom(tx) => tx.tx().gas_limit(),
        }
    }

    fn gas_price(&self) -> Option<u128> {
        match self {
            Self::Ethereum(tx) => tx.gas_price(),
            Self::Custom(tx) => tx.tx().gas_price(),
        }
    }

    fn max_fee_per_gas(&self) -> u128 {
        match self {
            Self::Ethereum(tx) => tx.max_fee_per_gas(),
            Self::Custom(tx) => tx.tx().max_fee_per_gas(),
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::Ethereum(tx) => tx.max_priority_fee_per_gas(),
            Self::Custom(tx) => tx.tx().max_priority_fee_per_gas(),
        }
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        match self {
            Self::Ethereum(tx) => tx.max_fee_per_blob_gas(),
            Self::Custom(tx) => tx.tx().max_fee_per_blob_gas(),
        }
    }

    fn priority_fee_or_price(&self) -> u128 {
        match self {
            Self::Ethereum(tx) => tx.priority_fee_or_price(),
            Self::Custom(tx) => tx.tx().priority_fee_or_price(),
        }
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        match self {
            Self::Ethereum(tx) => tx.effective_gas_price(base_fee),
            Self::Custom(tx) => tx.tx().effective_gas_price(base_fee),
        }
    }

    fn is_dynamic_fee(&self) -> bool {
        match self {
            Self::Ethereum(tx) => tx.is_dynamic_fee(),
            Self::Custom(tx) => tx.tx().is_dynamic_fee(),
        }
    }

    fn kind(&self) -> alloy_primitives::TxKind {
        match self {
            Self::Ethereum(tx) => tx.kind(),
            Self::Custom(tx) => tx.tx().kind(),
        }
    }

    fn is_create(&self) -> bool {
        match self {
            Self::Ethereum(tx) => tx.is_create(),
            Self::Custom(tx) => tx.tx().is_create(),
        }
    }

    fn value(&self) -> alloy_primitives::U256 {
        match self {
            Self::Ethereum(tx) => tx.value(),
            Self::Custom(tx) => tx.tx().value(),
        }
    }

    fn input(&self) -> &Bytes {
        match self {
            Self::Ethereum(tx) => tx.input(),
            Self::Custom(tx) => tx.tx().input(),
        }
    }

    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        match self {
            Self::Ethereum(tx) => tx.access_list(),
            Self::Custom(tx) => tx.tx().access_list(),
        }
    }

    fn blob_versioned_hashes(&self) -> Option<&[alloy_primitives::B256]> {
        match self {
            Self::Ethereum(tx) => tx.blob_versioned_hashes(),
            Self::Custom(tx) => tx.tx().blob_versioned_hashes(),
        }
    }

    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        match self {
            Self::Ethereum(tx) => tx.authorization_list(),
            Self::Custom(tx) => tx.tx().authorization_list(),
        }
    }
}

// Implement Typed2718 trait
impl Typed2718 for CustomTransaction {
    fn ty(&self) -> u8 {
        match self {
            Self::Ethereum(tx) => tx.ty(),
            Self::Custom(tx) => tx.tx().ty(),
        }
    }
}

// Implement Encodable2718 trait
impl Encodable2718 for CustomTransaction {
    fn encode_2718_len(&self) -> usize {
        match self {
            Self::Ethereum(tx) => tx.encode_2718_len(),
            Self::Custom(tx) => tx.encode_2718_len(),
        }
    }

    fn encode_2718(&self, out: &mut dyn BufMut) {
        match self {
            Self::Ethereum(tx) => tx.encode_2718(out),
            Self::Custom(tx) => tx.encode_2718(out),
        }
    }
}

// Implement Decodable2718 trait
impl Decodable2718 for CustomTransaction {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        if ty == TRANSFER_TX_TYPE_ID {
            Ok(Self::Custom(Signed::<TxPayment>::typed_decode(ty, buf)?))
        } else {
            Ok(Self::Ethereum(OpTxEnvelope::typed_decode(ty, buf)?))
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self::Ethereum(OpTxEnvelope::fallback_decode(buf)?))
    }
}

// Implement Decodable trait
impl Decodable for CustomTransaction {
    fn decode(buf: &mut &[u8]) -> RlpResult<Self> {
        Self::decode_2718(buf).map_err(|_| alloy_rlp::Error::Custom("Failed to decode".into()))
    }
}

// Implement Encodable trait
impl Encodable for CustomTransaction {
    fn encode(&self, out: &mut dyn BufMut) {
        self.encode_2718(out)
    }
}

// Implement SignerRecoverable trait
impl SignerRecoverable for CustomTransaction {
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        match self {
            Self::Ethereum(tx) => tx.recover_signer(),
            Self::Custom(tx) => {
                let signature_hash = tx.signature_hash();
                recover_signer(tx.signature(), signature_hash)
            }
        }
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        match self {
            Self::Ethereum(tx) => tx.recover_signer_unchecked(),
            Self::Custom(tx) => {
                let signature_hash = tx.signature_hash();
                recover_signer_unchecked(tx.signature(), signature_hash)
            }
        }
    }
}

// Implement additional traits that are specific to our custom transaction handling

impl SignedTransaction for CustomTransaction {
    fn tx_hash(&self) -> &TxHash {
        // This is a temporary workaround - we'll store the hash
        unimplemented!("tx_hash implementation requires caching")
    }

    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        match self {
            Self::Ethereum(tx) => tx.recover_signer_unchecked_with_buf(buf),
            Self::Custom(tx) => {
                tx.tx().encode_for_signing(buf);
                let signature_hash = keccak256(buf);
                recover_signer_unchecked(tx.signature(), signature_hash)
            }
        }
    }
}

impl InMemorySize for CustomTransaction {
    fn size(&self) -> usize {
        match self {
            Self::Ethereum(tx) => tx.size(),
            Self::Custom(tx) => tx.tx().size(),
        }
    }
}

impl FromTxCompact for CustomTransaction {
    type TxType = super::TxTypeCustom;

    fn from_tx_compact(buf: &[u8], tx_type: Self::TxType, signature: Signature) -> (Self, &[u8])
    where
        Self: Sized,
    {
        match tx_type {
            super::TxTypeCustom::Custom => {
                let (tx, buf) = TxPayment::from_compact(buf, buf.len());
                let tx = Signed::new_unhashed(tx, signature);
                (CustomTransaction::Custom(tx), buf)
            }
        }
    }
}

impl ToTxCompact for CustomTransaction {
    fn to_tx_compact(&self, buf: &mut (impl BufMut + AsMut<[u8]>)) {
        match self {
            Self::Ethereum(tx) => tx.to_tx_compact(buf),
            Self::Custom(tx) => {
                tx.tx().to_compact(buf);
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum BincodeCompatCustomTransaction {
    Ethereum(op_alloy_consensus::OpTxEnvelope),
    Custom(Signed<TxPayment>),
}

impl SerdeBincodeCompat for CustomTransaction {
    type BincodeRepr<'a> = BincodeCompatCustomTransaction;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        match self {
            Self::Ethereum(tx) => BincodeCompatCustomTransaction::Ethereum(tx.clone()),
            Self::Custom(tx) => BincodeCompatCustomTransaction::Custom(tx.clone()),
        }
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        match repr {
            BincodeCompatCustomTransaction::Ethereum(tx) => Self::Ethereum(tx),
            BincodeCompatCustomTransaction::Custom(tx) => Self::Custom(tx),
        }
    }
}

impl reth_codecs::alloy::transaction::Envelope for CustomTransaction {
    fn signature(&self) -> &Signature {
        match self {
            Self::Ethereum(tx) => tx.signature(),
            Self::Custom(tx) => tx.signature(),
        }
    }

    fn tx_type(&self) -> Self::TxType {
        super::TxTypeCustom::Custom
    }
}

impl Compact for CustomTransaction {
    fn to_compact<B>(&self, buf: &mut B) -> usize
    where
        B: BufMut + AsMut<[u8]>,
    {
        match self {
            Self::Ethereum(tx) => tx.to_compact(buf),
            Self::Custom(tx) => tx.tx().to_compact(buf),
        }
    }

    fn from_compact(buf: &[u8], len: usize) -> (Self, &[u8]) {
        let (signature, rest) = Signature::from_compact(buf, len);
        let (inner, buf) = <TxPayment as Compact>::from_compact(rest, len);
        let signed = Signed::new_unhashed(inner, signature);
        (CustomTransaction::Custom(signed), buf)
    }
}

impl OpTransaction for CustomTransaction {
    fn is_deposit(&self) -> bool {
        match self {
            Self::Ethereum(tx) => tx.is_deposit(),
            Self::Custom(_) => false,
        }
    }

    fn as_deposit(&self) -> Option<&Sealed<TxDeposit>> {
        None
    }
}