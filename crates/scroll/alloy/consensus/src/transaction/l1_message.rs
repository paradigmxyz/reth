//! Scroll L1 message transaction

use crate::ScrollTxType;
use std::vec::Vec;

use alloy_consensus::{Sealable, Transaction, Typed2718};
use alloy_eips::eip2718::{Decodable2718, Eip2718Error, Eip2718Result, Encodable2718};
use alloy_primitives::{
    keccak256,
    private::alloy_rlp::{Encodable, Header},
    Address, Bytes, ChainId, Signature, TxHash, TxKind, B256, U256,
};
use alloy_rlp::Decodable;
#[cfg(any(test, feature = "reth-codec"))]
use {reth_codecs::Compact, reth_codecs_derive::add_arbitrary_tests};

/// L1 message transaction type id, 0x7e in hex.
pub const L1_MESSAGE_TRANSACTION_TYPE: u8 = 126;

/// A message transaction sent from the settlement layer to the L2 for execution.
///
/// The signature of the L1 message is already verified on the L1 and as such doesn't contain
/// a signature field. Gas for the transaction execution on Scroll is already paid for on the L1.
///
/// # Bincode compatibility
///
/// `bincode` crate doesn't work with optionally serializable serde fields and some of the execution
/// types require optional serialization for RPC compatibility. Since `TxL1Message` doesn't
/// contain optionally serializable fields, no `bincode` compatible bridge implementation is
/// required.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "serde"), serde(rename_all = "camelCase"))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(any(test, feature = "reth-codec"), derive(Compact))]
#[cfg_attr(any(test, feature = "reth-codec"), add_arbitrary_tests(compact, rlp))]
pub struct TxL1Message {
    /// The queue index of the message in the L1 contract queue.
    #[cfg_attr(feature = "serde", serde(with = "alloy_serde::quantity"))]
    pub queue_index: u64,
    /// The gas limit for the transaction. Gas is paid for when message is sent from the L1.
    #[cfg_attr(feature = "serde", serde(with = "alloy_serde::quantity", rename = "gas"))]
    pub gas_limit: u64,
    /// The destination for the transaction. `Address` is used in place of `TxKind` since contract
    /// creations aren't allowed via L1 message transactions.
    pub to: Address,
    /// The value sent.
    pub value: U256,
    /// The L1 sender of the transaction.
    pub sender: Address,
    /// The input of the transaction.
    pub input: Bytes,
}

impl TxL1Message {
    /// Returns an empty signature for the [`TxL1Message`], which don't include a signature.
    pub const fn signature() -> Signature {
        Signature::new(U256::ZERO, U256::ZERO, false)
    }

    /// Decodes the inner [`TxL1Message`] fields from RLP bytes.
    ///
    /// NOTE: This assumes a RLP header has already been decoded, and _just_ decodes the following
    /// RLP fields in the following order:
    ///
    /// - `queue_index`
    /// - `gas_limit`
    /// - `to`
    /// - `value`
    /// - `input`
    /// - `sender`
    pub fn rlp_decode_fields(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self {
            queue_index: Decodable::decode(buf)?,
            gas_limit: Decodable::decode(buf)?,
            to: Decodable::decode(buf)?,
            value: Decodable::decode(buf)?,
            input: Decodable::decode(buf)?,
            sender: Decodable::decode(buf)?,
        })
    }

    /// Decodes the transaction from RLP bytes.
    pub fn rlp_decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let remaining = buf.len();

        let this = Self::rlp_decode_fields(buf)?;

        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: header.payload_length,
                got: remaining - buf.len(),
            });
        }

        Ok(this)
    }

    /// Outputs the length of the transaction's fields, without a RLP header.
    fn rlp_encoded_fields_length(&self) -> usize {
        self.queue_index.length() +
            self.gas_limit.length() +
            self.to.length() +
            self.value.length() +
            self.input.0.length() +
            self.sender.length()
    }

    /// Encode the fields of the transaction without a RLP header.
    /// <https://github.com/scroll-tech/go-ethereum/blob/9fff27e4f34fb5097100ed76ee725ce056267f4b/core/types/l1_message_tx.go#L12-L19>
    fn rlp_encode_fields(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.queue_index.encode(out);
        self.gas_limit.encode(out);
        self.to.encode(out);
        self.value.encode(out);
        self.input.encode(out);
        self.sender.encode(out);
    }

    pub(crate) const fn tx_type(&self) -> u8 {
        L1_MESSAGE_TRANSACTION_TYPE
    }

    /// Create a RLP header for the transaction.
    fn rlp_header(&self) -> Header {
        Header { list: true, payload_length: self.rlp_encoded_fields_length() }
    }

    /// RLP encodes the transaction.
    pub fn rlp_encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.rlp_header().encode(out);
        self.rlp_encode_fields(out);
    }

    /// Get the length of the transaction when RLP encoded.
    pub fn rlp_encoded_length(&self) -> usize {
        self.rlp_header().length_with_payload()
    }

    /// Get the length of the transaction when EIP-2718 encoded. This is the
    /// 1 byte type flag + the length of the RLP encoded transaction.
    pub fn eip2718_encoded_length(&self) -> usize {
        self.rlp_encoded_length() + 1
    }

    /// EIP-2718 encode the transaction.
    pub fn eip2718_encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        out.put_u8(L1_MESSAGE_TRANSACTION_TYPE);
        self.rlp_encode(out)
    }

    /// Calculates the in-memory size of the [`TxL1Message`] transaction.
    #[inline]
    pub fn size(&self) -> usize {
        size_of::<u64>() + // queue_index
            size_of::<u64>() + // gas_limit
            size_of::<Address>() + // to
            size_of::<U256>() + // value
            self.input.len() + // input
            size_of::<Address>() // sender
    }

    /// Calculates the hash of the [`TxL1Message`] transaction.
    pub fn tx_hash(&self) -> TxHash {
        let mut buf = Vec::with_capacity(self.eip2718_encoded_length());
        self.eip2718_encode(&mut buf);
        keccak256(&buf)
    }
}

impl Typed2718 for TxL1Message {
    fn ty(&self) -> u8 {
        ScrollTxType::L1Message as u8
    }
}

impl Encodable2718 for TxL1Message {
    fn type_flag(&self) -> Option<u8> {
        Some(self.tx_type())
    }

    fn encode_2718_len(&self) -> usize {
        self.eip2718_encoded_length()
    }

    fn encode_2718(&self, out: &mut dyn alloy_rlp::BufMut) {
        out.put_u8(self.tx_type());
        self.rlp_encode(out);
    }
}

impl Decodable2718 for TxL1Message {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        if ty != L1_MESSAGE_TRANSACTION_TYPE {
            return Err(Eip2718Error::UnexpectedType(ty));
        }
        let tx = Self::rlp_decode(buf)?;
        Ok(tx)
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        let tx = Self::decode(buf)?;
        Ok(tx)
    }
}

impl Encodable for TxL1Message {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.rlp_encode(out)
    }

    fn length(&self) -> usize {
        self.rlp_encoded_length()
    }
}

impl Decodable for TxL1Message {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::rlp_decode(buf)
    }
}

impl Transaction for TxL1Message {
    fn chain_id(&self) -> Option<ChainId> {
        None
    }

    fn nonce(&self) -> u64 {
        0u64
    }

    fn gas_limit(&self) -> u64 {
        self.gas_limit
    }

    fn gas_price(&self) -> Option<u128> {
        None
    }

    fn max_fee_per_gas(&self) -> u128 {
        0
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        None
    }

    fn max_fee_per_blob_gas(&self) -> Option<u128> {
        None
    }

    fn priority_fee_or_price(&self) -> u128 {
        0
    }

    fn effective_gas_price(&self, _base_fee: Option<u64>) -> u128 {
        0
    }

    fn is_dynamic_fee(&self) -> bool {
        false
    }

    fn kind(&self) -> TxKind {
        TxKind::Call(self.to)
    }

    fn is_create(&self) -> bool {
        false
    }

    fn value(&self) -> U256 {
        self.value
    }

    fn input(&self) -> &Bytes {
        &self.input
    }

    fn access_list(&self) -> Option<&alloy_eips::eip2930::AccessList> {
        None
    }

    fn blob_versioned_hashes(&self) -> Option<&[B256]> {
        None
    }

    fn authorization_list(&self) -> Option<&[alloy_eips::eip7702::SignedAuthorization]> {
        None
    }
}

impl Sealable for TxL1Message {
    fn hash_slow(&self) -> B256 {
        self.tx_hash()
    }
}

/// Scroll specific transaction fields
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "serde"), serde(rename_all = "camelCase"))]
pub struct ScrollL1MessageTransactionFields {
    /// The index of the transaction in the message queue.
    #[cfg_attr(any(test, feature = "serde"), serde(with = "alloy_serde::quantity"))]
    pub queue_index: u64,
    /// The sender of the transaction on the L1.
    pub sender: Address,
}

/// L1 message transactions don't have a signature, however, we include an empty signature in the
/// response for better compatibility.
///
/// This function can be used as `serialize_with` serde attribute for the [`TxL1Message`] and will
/// flatten [`TxL1Message::signature`] into response.
///
/// <https://github.com/scroll-tech/go-ethereum/blob/develop/core/types/l1_message_tx.go#L51>.
#[cfg(feature = "serde")]
pub fn serde_l1_message_tx_rpc<T: serde::Serialize, S: serde::Serializer>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::Serialize;

    #[derive(Serialize)]
    struct SerdeHelper<'a, T> {
        #[serde(flatten)]
        value: &'a T,
        #[serde(flatten)]
        signature: Signature,
    }

    SerdeHelper { value, signature: TxL1Message::signature() }.serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{address, bytes, hex, Bytes, U256};
    use alloy_rlp::BytesMut;
    use arbitrary::Arbitrary;
    use rand::Rng;
    use reth_codecs::{test_utils::UnusedBits, validate_bitflag_backwards_compat};

    #[test]
    fn test_rlp_roundtrip() {
        // <https://scrollscan.com/tx/0xace7103cc22a372c81cda04e15442a721cd3d5d64eda2e1578ba310d91597d97>
        let bytes = Bytes::from_static(&hex!("7ef9015a830e7991831e848094781e90f1c8fc4611c9b7497c3b47f99ef6969cbc80b901248ef1332e000000000000000000000000c186fa914353c44b2e33ebe05f21846f1048beda0000000000000000000000003bad7ad0728f9917d1bf08af5782dcbd516cdd96000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e799100000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000044493a4f8411b3f3d662006b9bf68884e71f1fc0f8ea04e4cb188354738202c3e34a473b93000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000947885bcbd5cecef1336b5300fb5186a12ddd8c478"));
        let tx_a = TxL1Message::decode(&mut bytes[1..].as_ref()).unwrap();
        let mut buf_a = BytesMut::default();
        tx_a.encode(&mut buf_a);
        assert_eq!(&buf_a[..], &bytes[1..]);
    }

    #[test]
    fn test_encode_decode_fields() {
        let original = TxL1Message {
            queue_index: 100,
            gas_limit: 0,
            to: Address::default(),
            value: U256::default(),
            sender: Address::default(),
            input: Bytes::default(),
        };
        let mut buffer = BytesMut::new();
        original.rlp_encode_fields(&mut buffer);
        let decoded = TxL1Message::rlp_decode_fields(&mut &buffer[..]).expect("Failed to decode");

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_encode_with_and_without_header() {
        let tx_deposit = TxL1Message {
            queue_index: 0,
            gas_limit: 50000,
            to: Address::default(),
            value: U256::default(),
            sender: Address::default(),
            input: Bytes::default(),
        };

        let mut buffer_with_header = BytesMut::new();
        tx_deposit.encode(&mut buffer_with_header);

        let mut buffer_without_header = BytesMut::new();
        tx_deposit.rlp_encode_fields(&mut buffer_without_header);

        assert!(buffer_with_header.len() > buffer_without_header.len());
    }

    #[test]
    fn test_payload_length() {
        let tx_deposit = TxL1Message {
            queue_index: 0,
            gas_limit: 50000,
            to: Address::default(),
            value: U256::default(),
            sender: Address::default(),
            input: Bytes::default(),
        };

        assert!(tx_deposit.size() > tx_deposit.rlp_encoded_fields_length());
    }

    #[test]
    fn test_deserialize_hex_to_u64() {
        let rpc_tx = r#"{"gas":"0x1e8480","input":"0x8ef1332e000000000000000000000000c186fa914353c44b2e33ebe05f21846f1048beda0000000000000000000000003bad7ad0728f9917d1bf08af5782dcbd516cdd96000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e7ba000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000044493a4f846ffc1507cbfe98a2b0ba1f06ea7e4eb749c001f78f6cb5540daa556a0566322a000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000","to":"0x781e90f1c8fc4611c9b7497c3b47f99ef6969cbc","value":"0x0","sender":"0x7885bcbd5cecef1336b5300fb5186a12ddd8c478","queueIndex":"0xe7ba0"}"#;
        // let obj: TxL1Message = serde_json::from_str(rpc_tx).unwrap();
        let obj = serde_json::from_str::<TxL1Message>(rpc_tx).unwrap();
        assert_eq!(obj.queue_index, 0xe7ba0);
    }

    #[test]
    fn test_bincode_roundtrip() {
        let mut bytes = [0u8; 1024];
        rand::rng().fill(bytes.as_mut_slice());
        let tx = TxL1Message::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap();

        let encoded = bincode::serialize(&tx).unwrap();
        let decoded: TxL1Message = bincode::deserialize(&encoded).unwrap();
        assert_eq!(decoded, tx);
    }

    #[test]
    fn test_eip2718_encode() {
        let tx =
            TxL1Message {
                queue_index: 947883,
                gas_limit: 2000000,
                to: address!("781e90f1c8fc4611c9b7497c3b47f99ef6969cbc"),
                value: U256::ZERO,
                sender: address!("7885bcbd5cecef1336b5300fb5186a12ddd8c478"),
                input: bytes!("8ef1332e000000000000000000000000c186fa914353c44b2e33ebe05f21846f1048beda0000000000000000000000003bad7ad0728f9917d1bf08af5782dcbd516cdd96000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e76ab00000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000044493a4f84f464e58d4bfa93bcc57abfb14dbe1b8ff46cd132b5709aab227f269727943d2f000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
            }
            ;
        let bytes = Bytes::from_static(&hex!("7ef9015a830e76ab831e848094781e90f1c8fc4611c9b7497c3b47f99ef6969cbc80b901248ef1332e000000000000000000000000c186fa914353c44b2e33ebe05f21846f1048beda0000000000000000000000003bad7ad0728f9917d1bf08af5782dcbd516cdd96000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000e76ab00000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000044493a4f84f464e58d4bfa93bcc57abfb14dbe1b8ff46cd132b5709aab227f269727943d2f000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000947885bcbd5cecef1336b5300fb5186a12ddd8c478"));

        let mut encoded = BytesMut::default();
        tx.encode_2718(&mut encoded);

        assert_eq!(encoded, bytes.as_ref())
    }

    #[test]
    fn test_compaction_backwards_compatibility() {
        assert_eq!(TxL1Message::bitflag_encoded_bytes(), 2);
        validate_bitflag_backwards_compat!(TxL1Message, UnusedBits::NotZero);
    }
}
