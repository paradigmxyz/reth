use alloy_consensus::{
    transaction::RlpEcdsaDecodableTx, Sealable, Sealed, Signed, Transaction, TxEip1559, TxEip2930,
    TxEip7702, TxLegacy, Typed2718,
};
use alloy_eips::{
    eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718},
    eip2930::AccessList,
    eip7702::SignedAuthorization,
};
use alloy_primitives::{Address, Bytes, Signature, TxKind, B256, U256};
use alloy_rlp::{Decodable, Encodable};

use crate::{ScrollTxType, TxL1Message};

/// The Ethereum [EIP-2718] Transaction Envelope, modified for Scroll chains.
///
/// # Note:
///
/// This enum distinguishes between tagged and untagged legacy transactions, as
/// the in-protocol merkle tree may commit to EITHER 0-prefixed or raw.
/// Therefore we must ensure that encoding returns the precise byte-array that
/// was decoded, preserving the presence or absence of the `TransactionType`
/// flag.
///
/// [EIP-2718]: https://eips.ethereum.org/EIPS/eip-2718
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(
    feature = "serde",
    serde(into = "serde_from::TaggedTxEnvelope", from = "serde_from::MaybeTaggedTxEnvelope")
)]
#[cfg_attr(all(any(test, feature = "arbitrary"), feature = "k256"), derive(arbitrary::Arbitrary))]
#[non_exhaustive]
pub enum ScrollTxEnvelope {
    /// An untagged [`TxLegacy`].
    Legacy(Signed<TxLegacy>),
    /// A [`TxEip2930`] tagged with type 1.
    Eip2930(Signed<TxEip2930>),
    /// A [`TxEip1559`] tagged with type 2.
    Eip1559(Signed<TxEip1559>),
    /// EIP-7702 transaction
    Eip7702(Signed<TxEip7702>),
    /// A [`TxL1Message`] tagged with type 0x7E.
    L1Message(Sealed<TxL1Message>),
}

impl From<Signed<TxLegacy>> for ScrollTxEnvelope {
    fn from(v: Signed<TxLegacy>) -> Self {
        Self::Legacy(v)
    }
}

impl From<Signed<TxEip2930>> for ScrollTxEnvelope {
    fn from(v: Signed<TxEip2930>) -> Self {
        Self::Eip2930(v)
    }
}

impl From<Signed<TxEip1559>> for ScrollTxEnvelope {
    fn from(v: Signed<TxEip1559>) -> Self {
        Self::Eip1559(v)
    }
}

impl From<Signed<TxEip7702>> for ScrollTxEnvelope {
    fn from(v: Signed<TxEip7702>) -> Self {
        Self::Eip7702(v)
    }
}

impl From<TxL1Message> for ScrollTxEnvelope {
    fn from(v: TxL1Message) -> Self {
        v.seal_slow().into()
    }
}

impl From<Sealed<TxL1Message>> for ScrollTxEnvelope {
    fn from(v: Sealed<TxL1Message>) -> Self {
        Self::L1Message(v)
    }
}

impl Typed2718 for ScrollTxEnvelope {
    fn ty(&self) -> u8 {
        match self {
            Self::Legacy(tx) => tx.tx().ty(),
            Self::Eip2930(tx) => tx.tx().ty(),
            Self::Eip1559(tx) => tx.tx().ty(),
            Self::Eip7702(tx) => tx.tx().ty(),
            Self::L1Message(tx) => tx.ty(),
        }
    }
}

impl Transaction for ScrollTxEnvelope {
    fn chain_id(&self) -> Option<u64> {
        match self {
            Self::Legacy(tx) => tx.tx().chain_id(),
            Self::Eip2930(tx) => tx.tx().chain_id(),
            Self::Eip1559(tx) => tx.tx().chain_id(),
            Self::Eip7702(tx) => tx.tx().chain_id(),
            Self::L1Message(tx) => tx.chain_id(),
        }
    }

    fn nonce(&self) -> u64 {
        match self {
            Self::Legacy(tx) => tx.tx().nonce(),
            Self::Eip2930(tx) => tx.tx().nonce(),
            Self::Eip1559(tx) => tx.tx().nonce(),
            Self::Eip7702(tx) => tx.tx().nonce(),
            Self::L1Message(tx) => tx.nonce(),
        }
    }

    fn gas_limit(&self) -> u64 {
        match self {
            Self::Legacy(tx) => tx.tx().gas_limit(),
            Self::Eip2930(tx) => tx.tx().gas_limit(),
            Self::Eip1559(tx) => tx.tx().gas_limit(),
            Self::Eip7702(tx) => tx.tx().gas_limit(),
            Self::L1Message(tx) => tx.gas_limit(),
        }
    }

    fn gas_price(&self) -> Option<u128> {
        match self {
            Self::Legacy(tx) => tx.tx().gas_price(),
            Self::Eip2930(tx) => tx.tx().gas_price(),
            Self::Eip1559(tx) => tx.tx().gas_price(),
            Self::Eip7702(tx) => tx.tx().gas_price(),
            Self::L1Message(tx) => tx.gas_price(),
        }
    }

    fn max_fee_per_gas(&self) -> u128 {
        match self {
            Self::Legacy(tx) => tx.tx().max_fee_per_gas(),
            Self::Eip2930(tx) => tx.tx().max_fee_per_gas(),
            Self::Eip1559(tx) => tx.tx().max_fee_per_gas(),
            Self::Eip7702(tx) => tx.tx().max_fee_per_gas(),
            Self::L1Message(tx) => tx.max_fee_per_gas(),
        }
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        match self {
            Self::Legacy(tx) => tx.tx().max_priority_fee_per_gas(),
            Self::Eip2930(tx) => tx.tx().max_priority_fee_per_gas(),
            Self::Eip1559(tx) => tx.tx().max_priority_fee_per_gas(),
            Self::Eip7702(tx) => tx.tx().max_priority_fee_per_gas(),
            Self::L1Message(tx) => tx.max_priority_fee_per_gas(),
        }
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        match self {
            Self::Legacy(tx) => tx.tx().max_fee_per_blob_gas(),
            Self::Eip2930(tx) => tx.tx().max_fee_per_blob_gas(),
            Self::Eip1559(tx) => tx.tx().max_fee_per_blob_gas(),
            Self::Eip7702(tx) => tx.tx().max_fee_per_blob_gas(),
            Self::L1Message(tx) => tx.max_fee_per_blob_gas(),
        }
    }

    fn priority_fee_or_price(&self) -> u128 {
        match self {
            Self::Legacy(tx) => tx.tx().priority_fee_or_price(),
            Self::Eip2930(tx) => tx.tx().priority_fee_or_price(),
            Self::Eip1559(tx) => tx.tx().priority_fee_or_price(),
            Self::Eip7702(tx) => tx.tx().priority_fee_or_price(),
            Self::L1Message(tx) => tx.priority_fee_or_price(),
        }
    }

    fn to(&self) -> Option<Address> {
        match self {
            Self::Legacy(tx) => tx.tx().to(),
            Self::Eip2930(tx) => tx.tx().to(),
            Self::Eip1559(tx) => tx.tx().to(),
            Self::Eip7702(tx) => tx.tx().to(),
            Self::L1Message(tx) => tx.to(),
        }
    }

    fn kind(&self) -> TxKind {
        match self {
            Self::Legacy(tx) => tx.tx().kind(),
            Self::Eip2930(tx) => tx.tx().kind(),
            Self::Eip1559(tx) => tx.tx().kind(),
            Self::Eip7702(tx) => tx.tx().kind(),
            Self::L1Message(tx) => tx.kind(),
        }
    }

    fn value(&self) -> U256 {
        match self {
            Self::Legacy(tx) => tx.tx().value(),
            Self::Eip2930(tx) => tx.tx().value(),
            Self::Eip1559(tx) => tx.tx().value(),
            Self::Eip7702(tx) => tx.tx().value(),
            Self::L1Message(tx) => tx.value(),
        }
    }

    fn input(&self) -> &Bytes {
        match self {
            Self::Legacy(tx) => tx.tx().input(),
            Self::Eip2930(tx) => tx.tx().input(),
            Self::Eip1559(tx) => tx.tx().input(),
            Self::Eip7702(tx) => tx.tx().input(),
            Self::L1Message(tx) => tx.input(),
        }
    }

    fn access_list(&self) -> Option<&AccessList> {
        match self {
            Self::Legacy(tx) => tx.tx().access_list(),
            Self::Eip2930(tx) => tx.tx().access_list(),
            Self::Eip1559(tx) => tx.tx().access_list(),
            Self::Eip7702(tx) => tx.tx().access_list(),
            Self::L1Message(tx) => tx.access_list(),
        }
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        match self {
            Self::Legacy(tx) => tx.tx().blob_versioned_hashes(),
            Self::Eip2930(tx) => tx.tx().blob_versioned_hashes(),
            Self::Eip1559(tx) => tx.tx().blob_versioned_hashes(),
            Self::Eip7702(tx) => tx.tx().blob_versioned_hashes(),
            Self::L1Message(tx) => tx.blob_versioned_hashes(),
        }
    }

    fn authorization_list(&self) -> Option<&[SignedAuthorization]> {
        match self {
            Self::Legacy(tx) => tx.tx().authorization_list(),
            Self::Eip2930(tx) => tx.tx().authorization_list(),
            Self::Eip1559(tx) => tx.tx().authorization_list(),
            Self::Eip7702(tx) => tx.tx().authorization_list(),
            Self::L1Message(tx) => tx.authorization_list(),
        }
    }

    fn is_dynamic_fee(&self) -> bool {
        match self {
            Self::Legacy(tx) => tx.tx().is_dynamic_fee(),
            Self::Eip2930(tx) => tx.tx().is_dynamic_fee(),
            Self::Eip1559(tx) => tx.tx().is_dynamic_fee(),
            Self::Eip7702(tx) => tx.tx().is_dynamic_fee(),
            Self::L1Message(tx) => tx.is_dynamic_fee(),
        }
    }

    fn effective_gas_price(&self, base_fee: Option<u64>) -> u128 {
        match self {
            Self::Legacy(tx) => tx.tx().effective_gas_price(base_fee),
            Self::Eip2930(tx) => tx.tx().effective_gas_price(base_fee),
            Self::Eip1559(tx) => tx.tx().effective_gas_price(base_fee),
            Self::Eip7702(tx) => tx.tx().effective_gas_price(base_fee),
            Self::L1Message(tx) => tx.effective_gas_price(base_fee),
        }
    }

    fn is_create(&self) -> bool {
        match self {
            Self::Legacy(tx) => tx.tx().is_create(),
            Self::Eip2930(tx) => tx.tx().is_create(),
            Self::Eip1559(tx) => tx.tx().is_create(),
            Self::Eip7702(tx) => tx.tx().is_create(),
            Self::L1Message(tx) => tx.is_create(),
        }
    }
}

impl ScrollTxEnvelope {
    /// Returns true if the transaction is a legacy transaction.
    #[inline]
    pub const fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy(_))
    }

    /// Returns true if the transaction is an EIP-2930 transaction.
    #[inline]
    pub const fn is_eip2930(&self) -> bool {
        matches!(self, Self::Eip2930(_))
    }

    /// Returns true if the transaction is an EIP-1559 transaction.
    #[inline]
    pub const fn is_eip1559(&self) -> bool {
        matches!(self, Self::Eip1559(_))
    }

    /// Returns true if the transaction is an EIP-7702 transaction.
    #[inline]
    pub const fn is_eip7702(&self) -> bool {
        matches!(self, Self::Eip7702(_))
    }

    /// Returns true if the transaction is a deposit transaction.
    #[inline]
    pub const fn is_l1_message(&self) -> bool {
        matches!(self, Self::L1Message(_))
    }

    /// Returns the [`TxLegacy`] variant if the transaction is a legacy transaction.
    pub const fn as_legacy(&self) -> Option<&Signed<TxLegacy>> {
        match self {
            Self::Legacy(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip2930`] variant if the transaction is an EIP-2930 transaction.
    pub const fn as_eip2930(&self) -> Option<&Signed<TxEip2930>> {
        match self {
            Self::Eip2930(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip1559`] variant if the transaction is an EIP-1559 transaction.
    pub const fn as_eip1559(&self) -> Option<&Signed<TxEip1559>> {
        match self {
            Self::Eip1559(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxEip7702`] variant if the transaction is an EIP-1559 transaction.
    pub const fn as_eip7702(&self) -> Option<&Signed<TxEip7702>> {
        match self {
            Self::Eip7702(tx) => Some(tx),
            _ => None,
        }
    }

    /// Returns the [`TxL1Message`] variant if the transaction is a deposit transaction.
    pub const fn as_l1_message(&self) -> Option<&Sealed<TxL1Message>> {
        match self {
            Self::L1Message(tx) => Some(tx),
            _ => None,
        }
    }

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

    /// Return the length of the inner txn, including type byte length
    pub fn eip2718_encoded_length(&self) -> usize {
        match self {
            Self::Legacy(t) => t.eip2718_encoded_length(),
            Self::Eip2930(t) => t.eip2718_encoded_length(),
            Self::Eip1559(t) => t.eip2718_encoded_length(),
            Self::Eip7702(t) => t.eip2718_encoded_length(),
            Self::L1Message(t) => t.eip2718_encoded_length(),
        }
    }

    /// Returns the signature for the transaction.
    pub const fn signature(&self) -> Option<Signature> {
        match self {
            Self::Legacy(t) => Some(*t.signature()),
            Self::Eip2930(t) => Some(*t.signature()),
            Self::Eip1559(t) => Some(*t.signature()),
            Self::Eip7702(t) => Some(*t.signature()),
            Self::L1Message(_) => None,
        }
    }
}

impl Encodable for ScrollTxEnvelope {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.network_encode(out)
    }

    fn length(&self) -> usize {
        self.network_len()
    }
}

impl Decodable for ScrollTxEnvelope {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self::network_decode(buf)?)
    }
}

impl Decodable2718 for ScrollTxEnvelope {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        match ty.try_into().map_err(|_| Eip2718Error::UnexpectedType(ty))? {
            ScrollTxType::Eip2930 => Ok(Self::Eip2930(TxEip2930::rlp_decode_signed(buf)?)),
            ScrollTxType::Eip1559 => Ok(Self::Eip1559(TxEip1559::rlp_decode_signed(buf)?)),
            ScrollTxType::Eip7702 => Ok(Self::Eip7702(TxEip7702::rlp_decode_signed(buf)?)),
            ScrollTxType::L1Message => Ok(Self::L1Message(TxL1Message::decode(buf)?.seal_slow())),
            ScrollTxType::Legacy => {
                Err(alloy_rlp::Error::Custom("type-0 eip2718 transactions are not supported")
                    .into())
            }
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self::Legacy(TxLegacy::rlp_decode_signed(buf)?))
    }
}

impl Encodable2718 for ScrollTxEnvelope {
    fn type_flag(&self) -> Option<u8> {
        match self {
            Self::Legacy(_) => None,
            Self::Eip2930(_) => Some(ScrollTxType::Eip2930 as u8),
            Self::Eip1559(_) => Some(ScrollTxType::Eip1559 as u8),
            Self::Eip7702(_) => Some(ScrollTxType::Eip7702 as u8),
            Self::L1Message(_) => Some(ScrollTxType::L1Message as u8),
        }
    }

    fn encode_2718_len(&self) -> usize {
        self.eip2718_encoded_length()
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        match self {
            // Legacy transactions have no difference between network and 2718
            Self::Legacy(tx) => tx.eip2718_encode(out),
            Self::Eip2930(tx) => {
                tx.eip2718_encode(out);
            }
            Self::Eip1559(tx) => {
                tx.eip2718_encode(out);
            }
            Self::Eip7702(tx) => {
                tx.eip2718_encode(out);
            }
            Self::L1Message(tx) => {
                tx.eip2718_encode(out);
            }
        }
    }

    fn trie_hash(&self) -> B256 {
        match self {
            Self::Legacy(tx) => *tx.hash(),
            Self::Eip2930(tx) => *tx.hash(),
            Self::Eip1559(tx) => *tx.hash(),
            Self::Eip7702(tx) => *tx.hash(),
            Self::L1Message(tx) => tx.seal(),
        }
    }
}

#[cfg(feature = "serde")]
mod serde_from {
    //! NB: Why do we need this?
    //!
    //! Because the tag may be missing, we need an abstraction over tagged (with
    //! type) and untagged (always legacy). This is [`MaybeTaggedTxEnvelope`].
    //!
    //! The tagged variant is [`TaggedTxEnvelope`], which always has a type tag.
    //!
    //! We serialize via [`TaggedTxEnvelope`] and deserialize via
    //! [`MaybeTaggedTxEnvelope`].
    use super::*;

    #[derive(Debug, serde::Deserialize)]
    #[serde(untagged)]
    pub(crate) enum MaybeTaggedTxEnvelope {
        Tagged(TaggedTxEnvelope),
        #[serde(with = "alloy_consensus::transaction::signed_legacy_serde")]
        Untagged(Signed<TxLegacy>),
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    #[serde(tag = "type")]
    pub(crate) enum TaggedTxEnvelope {
        #[serde(
            rename = "0x0",
            alias = "0x00",
            with = "alloy_consensus::transaction::signed_legacy_serde"
        )]
        Legacy(Signed<TxLegacy>),
        #[serde(rename = "0x1", alias = "0x01")]
        Eip2930(Signed<TxEip2930>),
        #[serde(rename = "0x2", alias = "0x02")]
        Eip1559(Signed<TxEip1559>),
        #[serde(rename = "0x4", alias = "0x04")]
        Eip7702(Signed<TxEip7702>),
        #[serde(
            rename = "0x7e",
            alias = "0x7E",
            serialize_with = "crate::serde_l1_message_tx_rpc"
        )]
        L1Message(Sealed<TxL1Message>),
    }

    impl From<MaybeTaggedTxEnvelope> for ScrollTxEnvelope {
        fn from(value: MaybeTaggedTxEnvelope) -> Self {
            match value {
                MaybeTaggedTxEnvelope::Tagged(tagged) => tagged.into(),
                MaybeTaggedTxEnvelope::Untagged(tx) => Self::Legacy(tx),
            }
        }
    }

    impl From<TaggedTxEnvelope> for ScrollTxEnvelope {
        fn from(value: TaggedTxEnvelope) -> Self {
            match value {
                TaggedTxEnvelope::Legacy(signed) => Self::Legacy(signed),
                TaggedTxEnvelope::Eip2930(signed) => Self::Eip2930(signed),
                TaggedTxEnvelope::Eip1559(signed) => Self::Eip1559(signed),
                TaggedTxEnvelope::Eip7702(signed) => Self::Eip7702(signed),
                TaggedTxEnvelope::L1Message(tx) => Self::L1Message(tx),
            }
        }
    }

    impl From<ScrollTxEnvelope> for TaggedTxEnvelope {
        fn from(value: ScrollTxEnvelope) -> Self {
            match value {
                ScrollTxEnvelope::Legacy(signed) => Self::Legacy(signed),
                ScrollTxEnvelope::Eip2930(signed) => Self::Eip2930(signed),
                ScrollTxEnvelope::Eip1559(signed) => Self::Eip1559(signed),
                ScrollTxEnvelope::Eip7702(signed) => Self::Eip7702(signed),
                ScrollTxEnvelope::L1Message(tx) => Self::L1Message(tx),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::vec;
    use alloy_primitives::{hex, Address, Bytes, U256};

    #[test]
    fn test_tx_gas_limit() {
        let tx = TxL1Message { gas_limit: 1, ..Default::default() };
        let tx_envelope = ScrollTxEnvelope::L1Message(tx.seal_slow());
        assert_eq!(tx_envelope.gas_limit(), 1);
    }

    #[test]
    fn test_encode_decode_l1_message() {
        let tx = TxL1Message {
            queue_index: 1,
            gas_limit: 2,
            to: Address::left_padding_from(&[3]),
            sender: Address::left_padding_from(&[4]),
            value: U256::from(4_u64),
            input: Bytes::from(vec![5]),
        };
        let tx_envelope = ScrollTxEnvelope::L1Message(tx.seal_slow());
        let encoded = tx_envelope.encoded_2718();
        let decoded = ScrollTxEnvelope::decode_2718(&mut encoded.as_ref()).unwrap();
        assert_eq!(encoded.len(), tx_envelope.encode_2718_len());
        assert_eq!(decoded, tx_envelope);
    }

    #[test]
    #[cfg(feature = "serde")]
    fn test_serde_roundtrip_l1_message() {
        let tx = TxL1Message {
            queue_index: 11,
            gas_limit: u64::MAX,
            sender: Address::random(),
            to: Address::random(),
            value: U256::MAX,
            input: Bytes::new(),
        };
        let tx_envelope = ScrollTxEnvelope::L1Message(tx.seal_slow());

        let serialized = serde_json::to_string(&tx_envelope).unwrap();
        let deserialized: ScrollTxEnvelope = serde_json::from_str(&serialized).unwrap();

        assert_eq!(tx_envelope, deserialized);
    }

    #[test]
    fn eip2718_l1_message_decode() {
        // <https://scrollscan.com/tx/0xace7103cc22a372c81cda04e15442a721cd3d5d64eda2e1578ba310d91597d97>
        let b = hex!("7ef9015a830e7991831e848094781e90f1c8fc4611c9b7497c3b47f99ef6969cbc80b901248ef1332e000000000000000000000000c186fa914353c44b2e33ebe05f21846f1048beda0000000000000000000000003bad7ad0728f9917d1bf08af5782dcbd516cdd96000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e799100000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000044493a4f8411b3f3d662006b9bf68884e71f1fc0f8ea04e4cb188354738202c3e34a473b93000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000947885bcbd5cecef1336b5300fb5186a12ddd8c478");

        let tx = ScrollTxEnvelope::decode_2718(&mut b[..].as_ref()).unwrap();
        tx.as_l1_message().unwrap();
    }

    #[test]
    fn eip1559_decode() {
        use alloy_consensus::SignableTransaction;
        use alloy_primitives::Signature;
        let tx = TxEip1559 {
            chain_id: 1u64,
            nonce: 2,
            max_fee_per_gas: 3,
            max_priority_fee_per_gas: 4,
            gas_limit: 5,
            to: Address::left_padding_from(&[6]).into(),
            value: U256::from(7_u64),
            input: vec![8].into(),
            access_list: Default::default(),
        };
        let sig = Signature::test_signature();
        let tx_signed = tx.into_signed(sig);
        let envelope: ScrollTxEnvelope = tx_signed.into();
        let encoded = envelope.encoded_2718();
        let mut slice = encoded.as_slice();
        let decoded = ScrollTxEnvelope::decode_2718(&mut slice).unwrap();
        assert!(matches!(decoded, ScrollTxEnvelope::Eip1559(_)));
    }
}
