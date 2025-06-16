use crate::{ScrollTxEnvelope, ScrollTxType, TxL1Message};
use alloy_consensus::{
    transaction::RlpEcdsaEncodableTx, SignableTransaction, Signed, Transaction, TxEip1559,
    TxEip2930, TxEip7702, TxLegacy, Typed2718,
};
use alloy_eips::{eip2930::AccessList, Encodable2718};
use alloy_primitives::{bytes::BufMut, Address, Bytes, ChainId, Signature, TxHash, TxKind, B256};
#[cfg(feature = "reth-codec")]
use {
    reth_codecs::{Compact, __private::bytes},
    reth_codecs_derive::generate_tests,
};

/// The `TypedTransaction` enum represents all Ethereum transaction request types, modified for
/// Scroll
///
/// Its variants correspond to specific allowed transactions:
/// 1. `Legacy` (pre-EIP2718) [`TxLegacy`]
/// 2. `EIP2930` (state access lists) [`TxEip2930`]
/// 3. `EIP1559` [`TxEip1559`]
/// 4. `Eip7702` [`TxEip7702`]
/// 5. `L1Message` [`TxL1Message`]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(
        from = "serde_from::MaybeTaggedTypedTransaction",
        into = "serde_from::TaggedTypedTransaction"
    )
)]
pub enum ScrollTypedTransaction {
    /// Legacy transaction
    Legacy(TxLegacy),
    /// EIP-2930 transaction
    Eip2930(TxEip2930),
    /// EIP-1559 transaction
    Eip1559(TxEip1559),
    /// EIP-7702 transaction
    Eip7702(TxEip7702),
    /// Scroll L1 message transaction
    L1Message(TxL1Message),
}

impl From<TxLegacy> for ScrollTypedTransaction {
    fn from(tx: TxLegacy) -> Self {
        Self::Legacy(tx)
    }
}

impl From<TxEip2930> for ScrollTypedTransaction {
    fn from(tx: TxEip2930) -> Self {
        Self::Eip2930(tx)
    }
}

impl From<TxEip1559> for ScrollTypedTransaction {
    fn from(tx: TxEip1559) -> Self {
        Self::Eip1559(tx)
    }
}

impl From<TxEip7702> for ScrollTypedTransaction {
    fn from(tx: TxEip7702) -> Self {
        Self::Eip7702(tx)
    }
}

impl From<TxL1Message> for ScrollTypedTransaction {
    fn from(tx: TxL1Message) -> Self {
        Self::L1Message(tx)
    }
}

impl From<ScrollTxEnvelope> for ScrollTypedTransaction {
    fn from(envelope: ScrollTxEnvelope) -> Self {
        match envelope {
            ScrollTxEnvelope::Legacy(tx) => Self::Legacy(tx.strip_signature()),
            ScrollTxEnvelope::Eip2930(tx) => Self::Eip2930(tx.strip_signature()),
            ScrollTxEnvelope::Eip1559(tx) => Self::Eip1559(tx.strip_signature()),
            ScrollTxEnvelope::Eip7702(tx) => Self::Eip7702(tx.strip_signature()),
            ScrollTxEnvelope::L1Message(tx) => Self::L1Message(tx.into_inner()),
        }
    }
}

impl ScrollTypedTransaction {
    /// Return the [`ScrollTxType`] of the inner txn.
    pub const fn tx_type(&self) -> ScrollTxType {
        match self {
            Self::Legacy(_) => ScrollTxType::Legacy,
            Self::Eip2930(_) => ScrollTxType::Eip2930,
            Self::Eip1559(_) => ScrollTxType::Eip1559,
            Self::Eip7702(_) => ScrollTxType::Eip7702,
            Self::L1Message(_) => ScrollTxType::L1Message,
        }
    }

    /// Return the inner legacy transaction if it exists.
    pub const fn legacy(&self) -> Option<&TxLegacy> {
        match self {
            Self::Legacy(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner EIP-2930 transaction if it exists.
    pub const fn eip2930(&self) -> Option<&TxEip2930> {
        match self {
            Self::Eip2930(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner EIP-1559 transaction if it exists.
    pub const fn eip1559(&self) -> Option<&TxEip1559> {
        match self {
            Self::Eip1559(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner EIP-1559 transaction if it exists.
    pub const fn eip7702(&self) -> Option<&TxEip7702> {
        match self {
            Self::Eip7702(tx) => Some(tx),
            _ => None,
        }
    }

    /// Return the inner l1 message if it exists.
    pub const fn l1_message(&self) -> Option<&TxL1Message> {
        match self {
            Self::L1Message(tx) => Some(tx),
            _ => None,
        }
    }

    /// Calculates the signing hash for the transaction.
    pub fn signature_hash(&self) -> B256 {
        match self {
            Self::Legacy(tx) => tx.signature_hash(),
            Self::Eip2930(tx) => tx.signature_hash(),
            Self::Eip1559(tx) => tx.signature_hash(),
            Self::Eip7702(tx) => tx.signature_hash(),
            Self::L1Message(_) => B256::ZERO,
        }
    }
}

impl Typed2718 for ScrollTypedTransaction {
    fn ty(&self) -> u8 {
        match self {
            Self::Legacy(_) => ScrollTxType::Legacy as u8,
            Self::Eip2930(_) => ScrollTxType::Eip2930 as u8,
            Self::Eip1559(_) => ScrollTxType::Eip1559 as u8,
            Self::Eip7702(_) => ScrollTxType::Eip7702 as u8,
            Self::L1Message(_) => ScrollTxType::L1Message as u8,
        }
    }
}

impl Transaction for ScrollTypedTransaction {
    fn chain_id(&self) -> Option<alloy_primitives::ChainId> {
        match self {
            Self::Legacy(tx) => tx.chain_id(),
            Self::Eip2930(tx) => tx.chain_id(),
            Self::Eip1559(tx) => tx.chain_id(),
            Self::Eip7702(tx) => tx.chain_id(),
            Self::L1Message(tx) => tx.chain_id(),
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            Self::Legacy(tx) => tx.nonce(),
            Self::Eip2930(tx) => tx.nonce(),
            Self::Eip1559(tx) => tx.nonce(),
            Self::Eip7702(tx) => tx.nonce(),
            Self::L1Message(tx) => tx.nonce(),
        }
    }

    fn gas_limit(&self) -> u64 {
        match self {
            Self::Legacy(tx) => tx.gas_limit(),
            Self::Eip2930(tx) => tx.gas_limit(),
            Self::Eip1559(tx) => tx.gas_limit(),
            Self::Eip7702(tx) => tx.gas_limit(),
            Self::L1Message(tx) => tx.gas_limit(),
        }
    }

    fn gas_price(&self) -> Option<u128> {
        match self {
            Self::Legacy(tx) => tx.gas_price(),
            Self::Eip2930(tx) => tx.gas_price(),
            Self::Eip1559(tx) => tx.gas_price(),
            Self::Eip7702(tx) => tx.gas_price(),
            Self::L1Message(tx) => tx.gas_price(),
        }
    }

    fn max_fee_per_gas(&self) -> u128 {
        match self {
            Self::Legacy(tx) => tx.max_fee_per_gas(),
            Self::Eip2930(tx) => tx.max_fee_per_gas(),
            Self::Eip1559(tx) => tx.max_fee_per_gas(),
            Self::Eip7702(tx) => tx.max_fee_per_gas(),
            Self::L1Message(tx) => tx.max_fee_per_gas(),
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::Legacy(tx) => tx.max_priority_fee_per_gas(),
            Self::Eip2930(tx) => tx.max_priority_fee_per_gas(),
            Self::Eip1559(tx) => tx.max_priority_fee_per_gas(),
            Self::Eip7702(tx) => tx.max_priority_fee_per_gas(),
            Self::L1Message(tx) => tx.max_priority_fee_per_gas(),
        }
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        match self {
            Self::Legacy(tx) => tx.max_fee_per_blob_gas(),
            Self::Eip2930(tx) => tx.max_fee_per_blob_gas(),
            Self::Eip1559(tx) => tx.max_fee_per_blob_gas(),
            Self::Eip7702(tx) => tx.max_fee_per_blob_gas(),
            Self::L1Message(tx) => tx.max_fee_per_blob_gas(),
        }
    }

    fn priority_fee_or_price(&self) -> u128 {
        match self {
            Self::Legacy(tx) => tx.priority_fee_or_price(),
            Self::Eip2930(tx) => tx.priority_fee_or_price(),
            Self::Eip1559(tx) => tx.priority_fee_or_price(),
            Self::Eip7702(tx) => tx.priority_fee_or_price(),
            Self::L1Message(tx) => tx.priority_fee_or_price(),
        }
    }

    fn to(&self) -> Option<Address> {
        match self {
            Self::Legacy(tx) => tx.to(),
            Self::Eip2930(tx) => tx.to(),
            Self::Eip1559(tx) => tx.to(),
            Self::Eip7702(tx) => tx.to(),
            Self::L1Message(tx) => tx.to(),
        }
    }

    fn kind(&self) -> TxKind {
        match self {
            Self::Legacy(tx) => tx.kind(),
            Self::Eip2930(tx) => tx.kind(),
            Self::Eip1559(tx) => tx.kind(),
            Self::Eip7702(tx) => tx.kind(),
            Self::L1Message(tx) => tx.kind(),
        }
    }

    fn value(&self) -> alloy_primitives::U256 {
        match self {
            Self::Legacy(tx) => tx.value(),
            Self::Eip2930(tx) => tx.value(),
            Self::Eip1559(tx) => tx.value(),
            Self::Eip7702(tx) => tx.value(),
            Self::L1Message(tx) => tx.value(),
        }
    }

    fn input(&self) -> &Bytes {
        match self {
            Self::Legacy(tx) => tx.input(),
            Self::Eip2930(tx) => tx.input(),
            Self::Eip1559(tx) => tx.input(),
            Self::Eip7702(tx) => tx.input(),
            Self::L1Message(tx) => tx.input(),
        }
    }

    fn access_list(&self) -> Option<&AccessList> {
        match self {
            Self::Legacy(tx) => tx.access_list(),
            Self::Eip2930(tx) => tx.access_list(),
            Self::Eip1559(tx) => tx.access_list(),
            Self::Eip7702(tx) => tx.access_list(),
            Self::L1Message(tx) => tx.access_list(),
        }
    }

    fn blob_versioned_hashes(&self) -> Option<&[alloy_primitives::B256]> {
        match self {
            Self::Legacy(tx) => tx.blob_versioned_hashes(),
            Self::Eip2930(tx) => tx.blob_versioned_hashes(),
            Self::Eip1559(tx) => tx.blob_versioned_hashes(),
            Self::Eip7702(tx) => tx.blob_versioned_hashes(),
            Self::L1Message(tx) => tx.blob_versioned_hashes(),
        }
    }

    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        match self {
            Self::Legacy(tx) => tx.authorization_list(),
            Self::Eip2930(tx) => tx.authorization_list(),
            Self::Eip1559(tx) => tx.authorization_list(),
            Self::Eip7702(tx) => tx.authorization_list(),
            Self::L1Message(tx) => tx.authorization_list(),
        }
    }

    fn is_dynamic_fee(&self) -> bool {
        match self {
            Self::Legacy(tx) => tx.is_dynamic_fee(),
            Self::Eip2930(tx) => tx.is_dynamic_fee(),
            Self::Eip1559(tx) => tx.is_dynamic_fee(),
            Self::Eip7702(tx) => tx.is_dynamic_fee(),
            Self::L1Message(tx) => tx.is_dynamic_fee(),
        }
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        match self {
            Self::Legacy(tx) => tx.effective_gas_price(base_fee),
            Self::Eip2930(tx) => tx.effective_gas_price(base_fee),
            Self::Eip1559(tx) => tx.effective_gas_price(base_fee),
            Self::Eip7702(tx) => tx.effective_gas_price(base_fee),
            Self::L1Message(tx) => tx.effective_gas_price(base_fee),
        }
    }

    fn is_create(&self) -> bool {
        match self {
            Self::Legacy(tx) => tx.is_create(),
            Self::Eip2930(tx) => tx.is_create(),
            Self::Eip1559(tx) => tx.is_create(),
            Self::Eip7702(tx) => tx.is_create(),
            Self::L1Message(tx) => tx.is_create(),
        }
    }
}

impl RlpEcdsaEncodableTx for ScrollTypedTransaction {
    fn rlp_encoded_fields_length(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip2930(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip1559(tx) => tx.rlp_encoded_fields_length(),
            Self::Eip7702(tx) => tx.rlp_encoded_fields_length(),
            Self::L1Message(tx) => tx.rlp_encoded_fields_length(),
        }
    }

    fn rlp_encode_fields(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            Self::Legacy(tx) => tx.rlp_encode_fields(out),
            Self::Eip2930(tx) => tx.rlp_encode_fields(out),
            Self::Eip1559(tx) => tx.rlp_encode_fields(out),
            Self::Eip7702(tx) => tx.rlp_encode_fields(out),
            Self::L1Message(tx) => tx.rlp_encode_fields(out),
        }
    }

    fn eip2718_encode_with_type(&self, signature: &Signature, _ty: u8, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip2930(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip1559(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::Eip7702(tx) => tx.eip2718_encode_with_type(signature, tx.ty(), out),
            Self::L1Message(tx) => tx.encode_2718(out),
        }
    }

    fn eip2718_encode(&self, signature: &Signature, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.eip2718_encode(signature, out),
            Self::Eip2930(tx) => tx.eip2718_encode(signature, out),
            Self::Eip1559(tx) => tx.eip2718_encode(signature, out),
            Self::Eip7702(tx) => tx.eip2718_encode(signature, out),
            Self::L1Message(tx) => tx.encode_2718(out),
        }
    }

    fn network_encode_with_type(&self, signature: &Signature, _ty: u8, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip2930(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip1559(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::Eip7702(tx) => tx.network_encode_with_type(signature, tx.ty(), out),
            Self::L1Message(tx) => tx.network_encode(out),
        }
    }

    fn network_encode(&self, signature: &Signature, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.network_encode(signature, out),
            Self::Eip2930(tx) => tx.network_encode(signature, out),
            Self::Eip1559(tx) => tx.network_encode(signature, out),
            Self::Eip7702(tx) => tx.network_encode(signature, out),
            Self::L1Message(tx) => tx.network_encode(out),
        }
    }

    fn tx_hash_with_type(&self, signature: &Signature, _ty: u8) -> TxHash {
        match self {
            Self::Legacy(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip2930(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip1559(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::Eip7702(tx) => tx.tx_hash_with_type(signature, tx.ty()),
            Self::L1Message(tx) => tx.tx_hash(),
        }
    }

    fn tx_hash(&self, signature: &Signature) -> TxHash {
        match self {
            Self::Legacy(tx) => tx.tx_hash(signature),
            Self::Eip2930(tx) => tx.tx_hash(signature),
            Self::Eip1559(tx) => tx.tx_hash(signature),
            Self::Eip7702(tx) => tx.tx_hash(signature),
            Self::L1Message(tx) => tx.tx_hash(),
        }
    }
}

impl SignableTransaction<Signature> for ScrollTypedTransaction {
    fn set_chain_id(&mut self, chain_id: ChainId) {
        match self {
            Self::Legacy(tx) => tx.set_chain_id(chain_id),
            Self::Eip2930(tx) => tx.set_chain_id(chain_id),
            Self::Eip1559(tx) => tx.set_chain_id(chain_id),
            Self::Eip7702(tx) => tx.set_chain_id(chain_id),
            Self::L1Message(_) => {}
        }
    }

    fn encode_for_signing(&self, out: &mut dyn BufMut) {
        match self {
            Self::Legacy(tx) => tx.encode_for_signing(out),
            Self::Eip2930(tx) => tx.encode_for_signing(out),
            Self::Eip1559(tx) => tx.encode_for_signing(out),
            Self::Eip7702(tx) => tx.encode_for_signing(out),
            Self::L1Message(_) => {}
        }
    }

    fn payload_len_for_signature(&self) -> usize {
        match self {
            Self::Legacy(tx) => tx.payload_len_for_signature(),
            Self::Eip2930(tx) => tx.payload_len_for_signature(),
            Self::Eip1559(tx) => tx.payload_len_for_signature(),
            Self::Eip7702(tx) => tx.payload_len_for_signature(),
            Self::L1Message(_) => 0,
        }
    }

    fn into_signed(self, signature: Signature) -> Signed<Self, Signature>
    where
        Self: Sized,
    {
        let hash = self.tx_hash(&signature);
        Signed::new_unchecked(self, signature, hash)
    }
}

#[cfg(feature = "reth-codec")]
impl Compact for ScrollTypedTransaction {
    fn to_compact<B>(&self, out: &mut B) -> usize
    where
        B: bytes::BufMut + AsMut<[u8]>,
    {
        let identifier = self.tx_type().to_compact(out);
        match self {
            Self::Legacy(tx) => tx.to_compact(out),
            Self::Eip2930(tx) => tx.to_compact(out),
            Self::Eip1559(tx) => tx.to_compact(out),
            Self::Eip7702(tx) => tx.to_compact(out),
            Self::L1Message(tx) => tx.to_compact(out),
        };
        identifier
    }

    fn from_compact(buf: &[u8], identifier: usize) -> (Self, &[u8]) {
        let (tx_type, buf) = ScrollTxType::from_compact(buf, identifier);
        match tx_type {
            ScrollTxType::Legacy => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::Legacy(tx), buf)
            }
            ScrollTxType::Eip2930 => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::Eip2930(tx), buf)
            }
            ScrollTxType::Eip1559 => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::Eip1559(tx), buf)
            }
            ScrollTxType::Eip7702 => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::Eip7702(tx), buf)
            }
            ScrollTxType::L1Message => {
                let (tx, buf) = Compact::from_compact(buf, buf.len());
                (Self::L1Message(tx), buf)
            }
        }
    }
}

#[cfg(feature = "reth-codec")]
generate_tests!(
    #[compact]
    ScrollTypedTransaction,
    ScrollTypedTransactionTests
);

#[cfg(feature = "serde")]
mod serde_from {
    //! NB: Why do we need this?
    //!
    //! Because the tag may be missing, we need an abstraction over tagged (with
    //! type) and untagged (always legacy). This is
    //! [`MaybeTaggedTypedTransaction`].
    //!
    //! The tagged variant is [`TaggedTypedTransaction`], which always has a
    //! type tag.
    //!
    //! We serialize via [`TaggedTypedTransaction`] and deserialize via
    //! [`MaybeTaggedTypedTransaction`].
    use super::*;

    #[derive(Debug, serde::Deserialize)]
    #[serde(untagged)]
    pub(crate) enum MaybeTaggedTypedTransaction {
        Tagged(TaggedTypedTransaction),
        Untagged(TxLegacy),
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    #[serde(tag = "type")]
    pub(crate) enum TaggedTypedTransaction {
        /// `Legacy` transaction
        #[serde(rename = "0x00", alias = "0x0")]
        Legacy(TxLegacy),
        /// `EIP-2930` transaction
        #[serde(rename = "0x01", alias = "0x1")]
        Eip2930(TxEip2930),
        /// `EIP-1559` transaction
        #[serde(rename = "0x02", alias = "0x2")]
        Eip1559(TxEip1559),
        /// `EIP-7702` transaction
        #[serde(rename = "0x04", alias = "0x4")]
        Eip7702(TxEip7702),
        /// `L1Message` transaction
        #[serde(
            rename = "0x7e",
            alias = "0x7E",
            serialize_with = "crate::serde_l1_message_tx_rpc"
        )]
        L1Message(TxL1Message),
    }

    impl From<MaybeTaggedTypedTransaction> for ScrollTypedTransaction {
        fn from(value: MaybeTaggedTypedTransaction) -> Self {
            match value {
                MaybeTaggedTypedTransaction::Tagged(tagged) => tagged.into(),
                MaybeTaggedTypedTransaction::Untagged(tx) => Self::Legacy(tx),
            }
        }
    }

    impl From<TaggedTypedTransaction> for ScrollTypedTransaction {
        fn from(value: TaggedTypedTransaction) -> Self {
            match value {
                TaggedTypedTransaction::Legacy(signed) => Self::Legacy(signed),
                TaggedTypedTransaction::Eip2930(signed) => Self::Eip2930(signed),
                TaggedTypedTransaction::Eip1559(signed) => Self::Eip1559(signed),
                TaggedTypedTransaction::Eip7702(signed) => Self::Eip7702(signed),
                TaggedTypedTransaction::L1Message(tx) => Self::L1Message(tx),
            }
        }
    }

    impl From<ScrollTypedTransaction> for TaggedTypedTransaction {
        fn from(value: ScrollTypedTransaction) -> Self {
            match value {
                ScrollTypedTransaction::Legacy(signed) => Self::Legacy(signed),
                ScrollTypedTransaction::Eip2930(signed) => Self::Eip2930(signed),
                ScrollTypedTransaction::Eip1559(signed) => Self::Eip1559(signed),
                ScrollTypedTransaction::Eip7702(signed) => Self::Eip7702(signed),
                ScrollTypedTransaction::L1Message(tx) => Self::L1Message(tx),
            }
        }
    }
}
