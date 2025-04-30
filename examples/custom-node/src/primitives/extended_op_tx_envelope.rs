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
    IsTyped2718, SignedTransaction,
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

/// A [`SignedTransaction`] implementation that combines two different transaction types.
///
/// This is intended to be used to extend existing presets, for example the ethereum or optstack
/// transaction types.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Hash, Eq, PartialEq)]
pub enum ExtendedTxEnvelope<BuiltIn, Other> {
    BuiltIn(BuiltIn),
    Other(Other),
}

pub type ExtendedOpTxEnvelope<T> = ExtendedTxEnvelope<OpTxEnvelope, T>;

impl<B, T> Transaction for ExtendedTxEnvelope<B, T>
where
    B: Transaction,
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

impl<B, T> IsTyped2718 for ExtendedTxEnvelope<B, T>
where
    B: IsTyped2718,
    T: IsTyped2718,
{
    fn is_type(type_id: u8) -> bool {
        B::is_type(type_id) || T::is_type(type_id)
    }
}

impl<B, T> InMemorySize for ExtendedTxEnvelope<B, T>
where
    B: InMemorySize,
    T: InMemorySize,
{
    fn size(&self) -> usize {
        delegate!(self => tx.size())
    }
}

impl<B, T> SignedTransaction for ExtendedTxEnvelope<B, T>
where
    B: SignedTransaction + SerdeBincodeCompat,
    T: SignedTransaction + SerdeBincodeCompat,
{
    fn tx_hash(&self) -> &TxHash {
        match self {
            Self::BuiltIn(tx) => tx.tx_hash(),
            Self::Other(tx) => tx.tx_hash(),
        }
    }

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
}

impl<B, T> Typed2718 for ExtendedTxEnvelope<B, T>
where
    B: Typed2718,
    T: Typed2718,
{
    fn ty(&self) -> u8 {
        match self {
            Self::BuiltIn(tx) => tx.ty(),
            Self::Other(tx) => tx.ty(),
        }
    }
}

impl<B, T> Decodable2718 for ExtendedTxEnvelope<B, T>
where
    B: Decodable2718 + SerdeBincodeCompat,
    T: Decodable2718 + SerdeBincodeCompat,
{
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into() {
            Ok(tx_type) => match tx_type {
                OpTxType::Eip2930 | OpTxType::Eip1559 | OpTxType::Eip7702 | OpTxType::Deposit => {
                    let envelope = B::typed_decode(ty, buf)?;
                    Ok(Self::BuiltIn(B::from_repr(envelope.as_repr())))
                }
                OpTxType::Legacy => {
                    let envelope = B::typed_decode(ty, buf)?;
                    Ok(Self::BuiltIn(B::from_repr(envelope.as_repr())))
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
                    let envelope = B::fallback_decode(buf)?;
                    Ok(Self::BuiltIn(B::from_repr(envelope.as_repr())))
                }
                OpTxType::Legacy => {
                    let envelope = B::fallback_decode(buf)?;
                    Ok(Self::BuiltIn(B::from_repr(envelope.as_repr())))
                }
            },
            Err(_) => {
                let other = T::fallback_decode(buf)?;
                Ok(Self::Other(other))
            }
        }
    }
}

impl<B, T> Encodable2718 for ExtendedTxEnvelope<B, T>
where
    B: Encodable2718,
    T: Encodable2718,
{
    fn encode_2718_len(&self) -> usize {
        match self {
            Self::BuiltIn(envelope) => envelope.encode_2718_len(),
            Self::Other(tx) => tx.encode_2718_len(),
        }
    }

    fn encode_2718(&self, out: &mut dyn BufMut) {
        match self {
            Self::BuiltIn(envelope) => envelope.encode_2718(out),
            Self::Other(tx) => tx.encode_2718(out),
        }
    }
}

impl<B, T> Encodable for ExtendedTxEnvelope<B, T>
where
    B: Encodable,
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

impl<B, T> Decodable for ExtendedTxEnvelope<B, T>
where
    B: Decodable,
    T: Decodable,
{
    fn decode(buf: &mut &[u8]) -> RlpResult<Self> {
        let original = *buf;

        match B::decode(buf) {
            Ok(tx) => Ok(Self::BuiltIn(tx)),
            Err(_) => {
                *buf = original;
                T::decode(buf).map(Self::Other)
            }
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum ExtendedTxEnvelopeRepr<'a, B: SerdeBincodeCompat, T: SerdeBincodeCompat> {
    BuiltIn(B::BincodeRepr<'a>),
    Other(T::BincodeRepr<'a>),
}

impl<B, T> SerdeBincodeCompat for ExtendedTxEnvelope<B, T>
where
    B: SerdeBincodeCompat + std::fmt::Debug,
    T: SerdeBincodeCompat + std::fmt::Debug,
{
    type BincodeRepr<'a> = ExtendedTxEnvelopeRepr<'a, B, T>;

    fn as_repr(&self) -> Self::BincodeRepr<'_> {
        match self {
            Self::BuiltIn(tx) => ExtendedTxEnvelopeRepr::BuiltIn(tx.as_repr()),
            Self::Other(tx) => ExtendedTxEnvelopeRepr::Other(tx.as_repr()),
        }
    }

    fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
        match repr {
            ExtendedTxEnvelopeRepr::BuiltIn(tx_repr) => Self::BuiltIn(B::from_repr(tx_repr)),
            ExtendedTxEnvelopeRepr::Other(tx_repr) => Self::Other(T::from_repr(tx_repr)),
        }
    }
}

impl<B, T> Compact for ExtendedTxEnvelope<B, T>
where
    B: Compact,
    T: Compact,
{
    fn to_compact<Buf>(&self, buf: &mut Buf) -> usize
    where
        Buf: alloy_rlp::bytes::BufMut + AsMut<[u8]>,
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
                let (tx, remaining) = B::from_compact(buf, len);
                return (Self::BuiltIn(tx), remaining);
            }
        }

        let (tx, remaining) = T::from_compact(buf, len);
        (Self::Other(tx), remaining)
    }
}
