#![allow(missing_docs, rustdoc::missing_crate_level_docs)]

use alloy_consensus::{crypto::SECP256K1N_HALF, EthereumTxEnvelope, Signed, TxEip4844, TxLegacy};
use alloy_eips::{
    eip2718::{Decodable2718, Encodable2718},
    eip2930::AccessList,
    eip7702::constants::EIP7702_TX_TYPE_ID,
};
use alloy_primitives::{keccak256, Address, Bytes, Signature, SignatureError, TxKind, B256, U256};
use std::fmt;

pub use alloy_eips::eip7702::SignedAuthorization as TransactionAuthorization;

mod codec;
mod full_width_rlp;
mod native;
mod snapshot;

pub use codec::{decode_transaction, decode_transactions, encode_transaction, encode_transactions};
pub use snapshot::TransactionSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyPayload {
    pub chain_id: Option<U256>,
    pub nonce: u64,
    pub gas_price: U256,
    pub gas_limit: u64,
    pub to: TxKind,
    pub value: U256,
    pub input: Bytes,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccessListPayload {
    pub chain_id: U256,
    pub nonce: u64,
    pub gas_price: U256,
    pub gas_limit: u64,
    pub to: TxKind,
    pub value: U256,
    pub input: Bytes,
    pub access_list: AccessList,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeeMarketPayload {
    pub chain_id: U256,
    pub nonce: u64,
    pub max_fee_per_gas: U256,
    pub max_priority_fee_per_gas: U256,
    pub gas_limit: u64,
    pub to: TxKind,
    pub value: U256,
    pub input: Bytes,
    pub access_list: AccessList,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobPayload {
    pub chain_id: U256,
    pub nonce: u64,
    pub max_fee_per_gas: U256,
    pub max_priority_fee_per_gas: U256,
    pub max_fee_per_blob_gas: U256,
    pub gas_limit: u64,
    pub to: Address,
    pub value: U256,
    pub input: Bytes,
    pub access_list: AccessList,
    pub blob_versioned_hashes: Vec<B256>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetCodePayload {
    pub chain_id: U256,
    pub nonce: u64,
    pub max_fee_per_gas: U256,
    pub max_priority_fee_per_gas: U256,
    pub gas_limit: u64,
    pub to: Address,
    pub value: U256,
    pub input: Bytes,
    pub access_list: AccessList,
    pub authorization_list: Vec<TransactionAuthorization>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransactionPayload {
    Legacy(LegacyPayload),
    AccessList(AccessListPayload),
    FeeMarket(FeeMarketPayload),
    Blob(BlobPayload),
    SetCode(SetCodePayload),
}

impl From<LegacyPayload> for TransactionPayload {
    fn from(value: LegacyPayload) -> Self {
        Self::Legacy(value)
    }
}

impl TransactionPayload {
    pub const fn as_legacy(&self) -> Option<&LegacyPayload> {
        if let Self::Legacy(value) = self {
            Some(value)
        } else {
            None
        }
    }

    pub const fn selector(&self) -> u8 {
        match self {
            Self::Legacy(p) => match (p.chain_id.is_some(), p.to) {
                (false, TxKind::Call(_)) => 1,
                (false, TxKind::Create) => 2,
                (true, TxKind::Call(_)) => 3,
                (true, TxKind::Create) => 4,
            },
            Self::AccessList(p) => {
                if matches!(p.to, TxKind::Create) {
                    6
                } else {
                    5
                }
            }
            Self::FeeMarket(p) => {
                if matches!(p.to, TxKind::Create) {
                    8
                } else {
                    7
                }
            }
            Self::Blob(_) => 9,
            Self::SetCode(_) => 10,
        }
    }

    pub const fn transaction_type(&self) -> u8 {
        match self {
            Self::Legacy(_) => 0,
            Self::AccessList(_) => 1,
            Self::FeeMarket(_) => 2,
            Self::Blob(_) => 3,
            Self::SetCode(_) => EIP7702_TX_TYPE_ID,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionSsz {
    payload: TransactionPayload,
    signature: Signature,
}

pub(crate) struct PayloadView<'a> {
    pub chain_id: Option<U256>,
    pub nonce: u64,
    pub fee: U256,
    pub blob_fee: Option<U256>,
    pub gas: u64,
    pub to: Option<Address>,
    pub value: U256,
    pub input: &'a [u8],
    pub access_list: Option<&'a AccessList>,
    pub priority_fee: Option<U256>,
    pub blob_hashes: Option<&'a [B256]>,
    pub authorizations: Option<&'a [TransactionAuthorization]>,
}

impl<'a> From<&'a TransactionPayload> for PayloadView<'a> {
    fn from(payload: &'a TransactionPayload) -> Self {
        match payload {
            TransactionPayload::Legacy(p) => Self {
                chain_id: p.chain_id,
                nonce: p.nonce,
                fee: p.gas_price,
                blob_fee: None,
                gas: p.gas_limit,
                to: p.to.into(),
                value: p.value,
                input: &p.input,
                access_list: None,
                priority_fee: None,
                blob_hashes: None,
                authorizations: None,
            },
            TransactionPayload::AccessList(p) => Self {
                chain_id: Some(p.chain_id),
                nonce: p.nonce,
                fee: p.gas_price,
                blob_fee: None,
                gas: p.gas_limit,
                to: p.to.into(),
                value: p.value,
                input: &p.input,
                access_list: Some(&p.access_list),
                priority_fee: None,
                blob_hashes: None,
                authorizations: None,
            },
            TransactionPayload::FeeMarket(p) => Self {
                chain_id: Some(p.chain_id),
                nonce: p.nonce,
                fee: p.max_fee_per_gas,
                blob_fee: None,
                gas: p.gas_limit,
                to: p.to.into(),
                value: p.value,
                input: &p.input,
                access_list: Some(&p.access_list),
                priority_fee: Some(p.max_priority_fee_per_gas),
                blob_hashes: None,
                authorizations: None,
            },
            TransactionPayload::Blob(p) => Self {
                chain_id: Some(p.chain_id),
                nonce: p.nonce,
                fee: p.max_fee_per_gas,
                blob_fee: Some(p.max_fee_per_blob_gas),
                gas: p.gas_limit,
                to: Some(p.to),
                value: p.value,
                input: &p.input,
                access_list: Some(&p.access_list),
                priority_fee: Some(p.max_priority_fee_per_gas),
                blob_hashes: Some(&p.blob_versioned_hashes),
                authorizations: None,
            },
            TransactionPayload::SetCode(p) => Self {
                chain_id: Some(p.chain_id),
                nonce: p.nonce,
                fee: p.max_fee_per_gas,
                blob_fee: None,
                gas: p.gas_limit,
                to: Some(p.to),
                value: p.value,
                input: &p.input,
                access_list: Some(&p.access_list),
                priority_fee: Some(p.max_priority_fee_per_gas),
                blob_hashes: None,
                authorizations: Some(&p.authorization_list),
            },
        }
    }
}

impl TransactionSsz {
    pub fn new(
        payload: impl Into<TransactionPayload>,
        signature: Signature,
    ) -> Result<Self, TransactionError> {
        validate_signature(&signature)?;
        let payload = payload.into();
        if let TransactionPayload::SetCode(p) = &payload &&
            p.authorization_list.is_empty()
        {
            return Err(TransactionError::InvalidEncoding("empty authorization list"));
        }
        let transaction = Self { payload, signature };
        transaction
            .signature
            .recover_address_from_prehash(&transaction.original_signing_hash()?)
            .map_err(TransactionError::Recovery)?;
        Ok(transaction)
    }

    pub fn from_alloy(signed: &Signed<TxLegacy>) -> Result<Self, TransactionError> {
        Self::from_envelope(&EthereumTxEnvelope::Legacy(signed.clone()))
    }

    pub fn from_rlp(bytes: &[u8]) -> Result<Self, TransactionError> {
        let mut remaining = bytes;
        let transaction = match EthereumTxEnvelope::<TxEip4844>::decode_2718(&mut remaining) {
            Ok(envelope) => {
                if !remaining.is_empty() {
                    return Err(TransactionError::TrailingBytes);
                }
                Self::from_envelope(&envelope)?
            }
            Err(error) => {
                let transaction = full_width_rlp::decode(bytes)?;
                match transaction.to_envelope() {
                    Err(TransactionError::UnsupportedWidth(_)) => transaction,
                    _ => return Err(TransactionError::Envelope(error)),
                }
            }
        };
        if transaction.original_rlp()?.as_ref() != bytes {
            return Err(TransactionError::NonCanonicalRlp);
        }

        Ok(transaction)
    }

    pub fn to_alloy(&self) -> Result<Signed<TxLegacy>, TransactionError> {
        match self.to_envelope()? {
            EthereumTxEnvelope::Legacy(signed) => Ok(signed),
            _ => Err(TransactionError::InvalidEncoding("expected legacy payload")),
        }
    }

    pub const fn payload(&self) -> &TransactionPayload {
        &self.payload
    }

    pub const fn signature(&self) -> &Signature {
        &self.signature
    }

    pub const fn selector(&self) -> u8 {
        self.payload.selector()
    }

    pub fn original_rlp(&self) -> Result<Bytes, TransactionError> {
        match self.to_envelope() {
            Ok(envelope) => Ok(Bytes::from(envelope.encoded_2718())),
            Err(TransactionError::UnsupportedWidth(_)) => {
                Ok(Bytes::from(full_width_rlp::encode(self, true)))
            }
            Err(error) => Err(error),
        }
    }

    pub fn original_transaction_hash(&self) -> Result<B256, TransactionError> {
        Ok(keccak256(self.original_rlp()?))
    }

    pub fn original_signing_hash(&self) -> Result<B256, TransactionError> {
        match self.to_envelope() {
            Ok(envelope) => Ok(envelope.signature_hash()),
            Err(TransactionError::UnsupportedWidth(_)) => {
                Ok(keccak256(full_width_rlp::encode(self, false)))
            }
            Err(error) => Err(error),
        }
    }

    pub fn from_envelope(source: &EthereumTxEnvelope<TxEip4844>) -> Result<Self, TransactionError> {
        let transaction = Self::new(native::payload(source), *source.signature())?;
        if transaction.original_transaction_hash()? != *source.tx_hash() {
            return Err(TransactionError::SourceHashMismatch);
        }
        Ok(transaction)
    }

    pub fn to_envelope(&self) -> Result<EthereumTxEnvelope<TxEip4844>, TransactionError> {
        native::envelope(self)
    }
}

pub fn convert_transactions(
    transactions: &[EthereumTxEnvelope<TxEip4844>],
) -> Result<Vec<TransactionSsz>, TransactionError> {
    transactions
        .iter()
        .enumerate()
        .map(|(index, transaction)| {
            TransactionSsz::from_envelope(transaction).map_err(|source| {
                TransactionError::AtTransaction { index, source: Box::new(source) }
            })
        })
        .collect()
}

#[derive(Debug)]
pub enum TransactionError {
    InvalidSignature,
    InvalidEncoding(&'static str),
    LengthOverflow,
    UnsupportedSelector(u8),
    UnsupportedTransactionType(u8),
    UnsupportedWidth(&'static str),
    SourceHashMismatch,
    NonCanonicalRlp,
    TrailingBytes,
    Rlp(alloy_rlp::Error),
    Recovery(SignatureError),
    Envelope(alloy_eips::eip2718::Eip2718Error),
    AtTransaction { index: usize, source: Box<Self> },
    Tree(reth_pureth_ssz::TreeConstructionError),
}

impl From<ssz::DecodeError> for TransactionError {
    fn from(_: ssz::DecodeError) -> Self {
        Self::InvalidEncoding("invalid SSZ encoding")
    }
}

impl fmt::Display for TransactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSignature => formatter.write_str("invalid transaction signature"),
            Self::InvalidEncoding(reason) => {
                write!(formatter, "invalid transaction encoding: {reason}")
            }
            Self::LengthOverflow => formatter.write_str("transaction encoding length overflow"),
            Self::UnsupportedSelector(selector) => {
                write!(formatter, "unsupported transaction selector {selector}")
            }
            Self::UnsupportedTransactionType(type_) => {
                write!(formatter, "unsupported Ethereum transaction type {type_}")
            }
            Self::UnsupportedWidth(field) => {
                write!(formatter, "{field} exceeds the supported Alloy execution width")
            }
            Self::SourceHashMismatch => formatter.write_str("source transaction hash mismatch"),
            Self::NonCanonicalRlp => formatter.write_str("non-canonical transaction RLP"),
            Self::TrailingBytes => formatter.write_str("trailing transaction RLP bytes"),
            Self::Rlp(error) => write!(formatter, "transaction RLP decoding failed: {error}"),
            Self::Recovery(error) => {
                write!(formatter, "transaction signer recovery failed: {error}")
            }
            Self::Tree(error) => write!(formatter, "transaction tree construction failed: {error}"),
            Self::Envelope(error) => {
                write!(formatter, "native transaction decoding failed: {error}")
            }
            Self::AtTransaction { index, source } => {
                write!(formatter, "transaction {index}: {source}")
            }
        }
    }
}

impl std::error::Error for TransactionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rlp(error) => Some(error),
            Self::Recovery(error) => Some(error),
            Self::Tree(error) => Some(error),
            Self::Envelope(error) => Some(error),
            Self::AtTransaction { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

fn validate_signature(signature: &Signature) -> Result<(), TransactionError> {
    let order = (SECP256K1N_HALF << 1) + U256::from(1);

    if signature.r() == U256::ZERO ||
        signature.r() >= order ||
        signature.s() == U256::ZERO ||
        signature.s() > SECP256K1N_HALF
    {
        return Err(TransactionError::InvalidSignature);
    }

    Ok(())
}

#[cfg(test)]
mod tests;
