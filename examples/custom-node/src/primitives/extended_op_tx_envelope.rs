use alloy_consensus::Transaction;
use alloy_eips::{
    eip2718::{Eip2718Error, Eip2718Result},
    eip2930::AccessList,
    eip7702::SignedAuthorization,
    Decodable2718, Encodable2718, Typed2718,
};
use alloy_primitives::{ChainId, TxHash};
use alloy_rlp::{BufMut, Decodable, Encodable, Result as RlpResult};
use op_alloy_consensus::{OpTxEnvelope, OpTxType};
use reth_codecs::Compact;
use reth_ethereum::primitives::{
    serde_bincode_compat::SerdeBincodeCompat, transaction::signed::RecoveryError, InMemorySize,
    SignedTransaction,
};
use revm_primitives::{Address, Bytes, TxKind, B256, U256};
use std::convert::TryInto;

macro_rules! delegate {
    ($self:expr => $tx:ident.$method:ident($($arg:expr),*)) => {
        match $self {
            Self::BuiltIn($tx) => $tx.$method($($arg),*),
            Self::Other($tx) => $tx.$method($($arg),*),
        }
    };
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Hash, Eq, PartialEq)]
pub enum ExtendedOpTxEnvelope<T> {
    BuiltIn(Box<OpTxEnvelope>),
    Other(T),
}

impl<T> Transaction for ExtendedOpTxEnvelope<T>
where
    T: Transaction,
{
    fn chain_id(&self) -> Option<ChainId> {
        delegate!(self => tx.chain_id())
    }

    fn nonce(&self) -> u64 {
        delegate!(self => tx.nonce())
    }

    fn gas_limit(&self) -> u64 {
        delegate!(self => tx.gas_limit())
    }

    fn gas_price(&self) -> Option<u128> {
        delegate!(self => tx.gas_price())
    }

    fn max_fee_per_gas(&self) -> u128 {
        delegate!(self => tx.max_fee_per_gas())
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        delegate!(self => tx.max_priority_fee_per_gas())
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        delegate!(self => tx.max_fee_per_blob_gas())
    }

    fn priority_fee_or_price(&self) -> u128 {
        delegate!(self => tx.priority_fee_or_price())
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        delegate!(self => tx.effective_gas_price(base_fee))
    }

    fn is_dynamic_fee(&self) -> bool {
        delegate!(self => tx.is_dynamic_fee())
    }

    fn kind(&self) -> TxKind {
        delegate!(self => tx.kind())
    }

    fn is_create(&self) -> bool {
        match self {
            Self::BuiltIn(tx) => tx.is_create(),
            Self::Other(_tx) => false,
        }
    }

    fn value(&self) -> U256 {
        delegate!(self => tx.value())
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
        delegate!(self => tx.access_list())
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        delegate!(self => tx.blob_versioned_hashes())
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        delegate!(self => tx.authorization_list())
    }
}

impl<T> InMemorySize for ExtendedOpTxEnvelope<T>
where
    T: InMemorySize,
{
    fn size(&self) -> usize {
        delegate!(self => tx.size())
    }
}

impl<T> SignedTransaction for ExtendedOpTxEnvelope<T>
where
    T: SignedTransaction,
{
    fn recover_signer(&self) -> Result<Address, RecoveryError> {
        delegate!(self => tx.recover_signer())
    }

    fn recover_signer_unchecked(&self) -> Result<Address, RecoveryError> {
        delegate!(self => tx.recover_signer_unchecked())
    }

    fn recover_signer_unchecked_with_buf(
        &self,
        buf: &mut Vec<u8>,
    ) -> Result<Address, RecoveryError> {
        delegate!(self => tx.recover_signer_unchecked_with_buf(buf))
    }

    fn tx_hash(&self) -> &TxHash {
        match self {
            Self::BuiltIn(tx) => match &**tx {
                OpTxEnvelope::Legacy(tx) => tx.hash(),
                OpTxEnvelope::Eip1559(tx) => tx.hash(),
                OpTxEnvelope::Eip2930(tx) => tx.hash(),
                OpTxEnvelope::Eip7702(tx) => tx.hash(),
                OpTxEnvelope::Deposit(tx) => tx.hash_ref(),
            },
            Self::Other(tx) => tx.tx_hash(),
        }
    }
}

impl<T> Typed2718 for ExtendedOpTxEnvelope<T>
where
    T: Typed2718,
{
    fn ty(&self) -> u8 {
        match self {
            Self::BuiltIn(tx) => tx.ty(),
            Self::Other(tx) => tx.ty(),
        }
    }
}

impl<T> Decodable2718 for ExtendedOpTxEnvelope<T>
where
    T: Decodable2718,
{
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into() {
            Ok(tx_type) => match tx_type {
                OpTxType::Eip2930 | OpTxType::Eip1559 | OpTxType::Eip7702 | OpTxType::Deposit => {
                    let envelope = OpTxEnvelope::typed_decode(ty, buf)?;
                    Ok(Self::BuiltIn(Box::new(envelope)))
                }
                OpTxType::Legacy => {
                    let envelope = OpTxEnvelope::typed_decode(ty, buf)?;
                    Ok(Self::BuiltIn(Box::new(envelope)))
                }
            },
            Err(_) => {
                let other = T::typed_decode(ty, buf)?;
                Ok(Self::Other(other))
            }
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        if buf.is_empty() {
            return Err(Eip2718Error::RlpError(alloy_rlp::Error::InputTooShort));
        }

        let type_byte = buf[0];
        match type_byte.try_into() {
            Ok(tx_type) => match tx_type {
                OpTxType::Eip2930 | OpTxType::Eip1559 | OpTxType::Eip7702 | OpTxType::Deposit => {
                    let envelope = OpTxEnvelope::fallback_decode(buf)?;
                    Ok(Self::BuiltIn(Box::new(envelope)))
                }
                OpTxType::Legacy => {
                    let envelope = OpTxEnvelope::fallback_decode(buf)?;
                    Ok(Self::BuiltIn(Box::new(envelope)))
                }
            },
            Err(_) => {
                let other = T::fallback_decode(buf)?;
                Ok(Self::Other(other))
            }
        }
    }
}

impl<T> Encodable2718 for ExtendedOpTxEnvelope<T>
where
    T: Encodable2718,
{
    fn encode_2718(&self, out: &mut dyn BufMut) {
        match self {
            Self::BuiltIn(envelope) => envelope.encode_2718(out),
            Self::Other(tx) => tx.encode_2718(out),
        }
    }

    fn encode_2718_len(&self) -> usize {
        match self {
            Self::BuiltIn(envelope) => envelope.encode_2718_len(),
            Self::Other(tx) => tx.encode_2718_len(),
        }
    }
}

impl<T> Encodable for ExtendedOpTxEnvelope<T>
where
    T: Encodable,
{
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::BuiltIn(envelope) => envelope.encode(out),
            Self::Other(tx) => tx.encode(out),
        }
    }

    fn length(&self) -> usize {
        match self {
            Self::BuiltIn(envelope) => envelope.length(),
            Self::Other(tx) => tx.length(),
        }
    }
}

impl<T> Decodable for ExtendedOpTxEnvelope<T>
where
    T: Decodable,
{
    fn decode(buf: &mut &[u8]) -> RlpResult<Self> {
        OpTxEnvelope::decode(buf)
            .map(|tx_envelope| Self::BuiltIn(Box::new(tx_envelope)))
            .or_else(|_| T::decode(buf).map(Self::Other))
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum ExtendedOpTxEnvelopeRepr<'a, T: SerdeBincodeCompat> {
    BuiltIn(Box<OpTxEnvelope>),
    Other(T::BincodeRepr<'a>),
}

impl<T> SerdeBincodeCompat for ExtendedOpTxEnvelope<T>
where
    T: SerdeBincodeCompat + std::fmt::Debug,
{
    type BincodeRepr<'a> = ExtendedOpTxEnvelopeRepr<'a, T>;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        match self {
            // since OpTxEnvelope doesn't implement SerdeBincodeCompat yet,
            // we need to clone the envelope for now
            // TODO: use as_repr once https://github.com/paradigmxyz/reth/issues/15377 is done
            Self::BuiltIn(tx) => ExtendedOpTxEnvelopeRepr::BuiltIn(tx.clone()),
            Self::Other(tx) => ExtendedOpTxEnvelopeRepr::Other(tx.as_repr()),
        }
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        match repr {
            ExtendedOpTxEnvelopeRepr::BuiltIn(tx) => Self::BuiltIn(tx),
            ExtendedOpTxEnvelopeRepr::Other(tx_repr) => Self::Other(T::from_repr(tx_repr)),
        }
    }
}

impl<T> Compact for ExtendedOpTxEnvelope<T>
where
    T: Compact,
{
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
        if !buf.is_empty() {
            let type_byte = buf[0];
            if let Ok(_tx_type) = <u8 as TryInto<OpTxType>>::try_into(type_byte) {
                let (tx, remaining) = OpTxEnvelope::from_compact(buf, len);
                return (Self::BuiltIn(Box::new(tx)), remaining);
            }
        }

        let (tx, remaining) = T::from_compact(buf, len);
        (Self::Other(tx), remaining)
    }
}
