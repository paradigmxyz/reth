use core::fmt::Debug;

use alloc::vec::Vec;
use alloy_consensus::{
    Eip2718EncodableReceipt, Eip658Value, ReceiptWithBloom, RlpDecodableReceipt,
    RlpEncodableReceipt, TxReceipt, TxType, Typed2718,
};
use alloy_eips::{
    eip2718::{Eip2718Error, Eip2718Result, Encodable2718, IsTyped2718},
    Decodable2718,
};
use alloy_primitives::{Bloom, Log, B256};
use alloy_rlp::{BufMut, Decodable, Encodable, Header};
use reth_primitives_traits::{proofs::ordered_trie_root_with_encoder, InMemorySize};

/// Helper trait alias with requirements for transaction type generic to be used within [`Receipt`].
pub trait TxTy:
    Debug
    + Copy
    + Eq
    + Send
    + Sync
    + InMemorySize
    + Typed2718
    + TryFrom<u8, Error = Eip2718Error>
    + Decodable
    + 'static
{
}
impl<T> TxTy for T where
    T: Debug
        + Copy
        + Eq
        + Send
        + Sync
        + InMemorySize
        + Typed2718
        + TryFrom<u8, Error = Eip2718Error>
        + Decodable
        + 'static
{
}

/// Typed ethereum transaction receipt.
/// Receipt containing result of transaction execution.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "reth-codec", reth_codecs::add_arbitrary_tests(compact, rlp))]
pub struct Receipt<T = TxType> {
    /// Receipt type.
    pub tx_type: T,
    /// If transaction is executed successfully.
    ///
    /// This is the `statusCode`
    pub success: bool,
    /// Gas used
    pub cumulative_gas_used: u64,
    /// Log send from contracts.
    pub logs: Vec<Log>,
}

impl<T: TxTy> Receipt<T> {
    /// Returns length of RLP-encoded receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encoded_fields_length(&self, bloom: &Bloom) -> usize {
        self.success.length() +
            self.cumulative_gas_used.length() +
            bloom.length() +
            self.logs.length()
    }

    /// RLP-encodes receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encode_fields(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        self.success.encode(out);
        self.cumulative_gas_used.encode(out);
        bloom.encode(out);
        self.logs.encode(out);
    }

    /// Returns RLP header for inner encoding.
    pub fn rlp_header_inner(&self, bloom: &Bloom) -> Header {
        Header { list: true, payload_length: self.rlp_encoded_fields_length(bloom) }
    }

    /// RLP-decodes the receipt from the provided buffer. This does not expect a type byte or
    /// network header.
    pub fn rlp_decode_inner(
        buf: &mut &[u8],
        tx_type: T,
    ) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }

        let remaining = buf.len();

        let success = Decodable::decode(buf)?;
        let cumulative_gas_used = Decodable::decode(buf)?;
        let logs_bloom = Decodable::decode(buf)?;
        let logs = Decodable::decode(buf)?;

        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok(ReceiptWithBloom {
            receipt: Self { cumulative_gas_used, tx_type, success, logs },
            logs_bloom,
        })
    }

    /// Calculates the receipt root for a header for the reference type of [Receipt].
    ///
    /// NOTE: Prefer `proofs::calculate_receipt_root` if you have log blooms memoized.
    pub fn calculate_receipt_root_no_memo(receipts: &[Self]) -> B256 {
        ordered_trie_root_with_encoder(receipts, |r, buf| r.with_bloom_ref().encode_2718(buf))
    }

    /// Returns length of RLP-encoded receipt fields without the given [`Bloom`] without an RLP
    /// header
    pub fn rlp_encoded_fields_length_without_bloom(&self) -> usize {
        self.success.length() + self.cumulative_gas_used.length() + self.logs.length()
    }

    /// RLP-encodes receipt fields without the given [`Bloom`] without an RLP header.
    pub fn rlp_encode_fields_without_bloom(&self, out: &mut dyn BufMut) {
        self.success.encode(out);
        self.cumulative_gas_used.encode(out);
        self.logs.encode(out);
    }

    /// Returns RLP header for inner encoding.
    pub fn rlp_header_inner_without_bloom(&self) -> Header {
        Header { list: true, payload_length: self.rlp_encoded_fields_length_without_bloom() }
    }

    /// RLP-decodes the receipt from the provided buffer. This does not expect a type byte or
    /// network header.
    pub fn rlp_decode_inner_without_bloom(buf: &mut &[u8], tx_type: T) -> alloy_rlp::Result<Self> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }

        let remaining = buf.len();
        let success = Decodable::decode(buf)?;
        let cumulative_gas_used = Decodable::decode(buf)?;
        let logs = Decodable::decode(buf)?;

        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok(Self { tx_type, success, cumulative_gas_used, logs })
    }
}

impl<T: TxTy> Eip2718EncodableReceipt for Receipt<T> {
    fn eip2718_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        !self.tx_type.is_legacy() as usize + self.rlp_header_inner(bloom).length_with_payload()
    }

    fn eip2718_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        if !self.tx_type.is_legacy() {
            out.put_u8(self.tx_type.ty());
        }
        self.rlp_header_inner(bloom).encode(out);
        self.rlp_encode_fields(bloom, out);
    }
}

impl<T: TxTy> RlpEncodableReceipt for Receipt<T> {
    fn rlp_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        let mut len = self.eip2718_encoded_length_with_bloom(bloom);
        if !self.tx_type.is_legacy() {
            len += Header {
                list: false,
                payload_length: self.eip2718_encoded_length_with_bloom(bloom),
            }
            .length();
        }

        len
    }

    fn rlp_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        if !self.tx_type.is_legacy() {
            Header { list: false, payload_length: self.eip2718_encoded_length_with_bloom(bloom) }
                .encode(out);
        }
        self.eip2718_encode_with_bloom(bloom, out);
    }
}

impl<T: TxTy> RlpDecodableReceipt for Receipt<T> {
    fn rlp_decode_with_bloom(buf: &mut &[u8]) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let header_buf = &mut &**buf;
        let header = Header::decode(header_buf)?;

        // Legacy receipt, reuse initial buffer without advancing
        if header.list {
            return Self::rlp_decode_inner(buf, T::try_from(0)?)
        }

        // Otherwise, advance the buffer and try decoding type flag followed by receipt
        *buf = *header_buf;

        let remaining = buf.len();
        let tx_type = T::decode(buf)?;
        let this = Self::rlp_decode_inner(buf, tx_type)?;

        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok(this)
    }
}

impl<T: TxTy> Encodable2718 for Receipt<T> {
    fn encode_2718_len(&self) -> usize {
        (!self.tx_type.is_legacy() as usize) +
            self.rlp_header_inner_without_bloom().length_with_payload()
    }

    // encode the header
    fn encode_2718(&self, out: &mut dyn BufMut) {
        if !self.tx_type.is_legacy() {
            out.put_u8(self.tx_type.ty());
        }
        self.rlp_header_inner_without_bloom().encode(out);
        self.rlp_encode_fields_without_bloom(out);
    }
}

impl<T: TxTy> Decodable2718 for Receipt<T> {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self::rlp_decode_inner_without_bloom(buf, T::try_from(ty)?)?)
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self::rlp_decode_inner_without_bloom(buf, T::try_from(0)?)?)
    }
}

impl<T: TxTy> Encodable for Receipt<T> {
    fn encode(&self, out: &mut dyn BufMut) {
        self.network_encode(out);
    }

    fn length(&self) -> usize {
        self.network_len()
    }
}

impl<T: TxTy> Decodable for Receipt<T> {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Ok(Self::network_decode(buf)?)
    }
}

impl<T: TxTy> TxReceipt for Receipt<T> {
    type Log = Log;

    fn status_or_post_state(&self) -> Eip658Value {
        self.success.into()
    }

    fn status(&self) -> bool {
        self.success
    }

    fn bloom(&self) -> Bloom {
        alloy_primitives::logs_bloom(self.logs())
    }

    fn cumulative_gas_used(&self) -> u64 {
        self.cumulative_gas_used
    }

    fn logs(&self) -> &[Log] {
        &self.logs
    }

    fn into_logs(self) -> Vec<Log> {
        self.logs
    }
}

impl<T: TxTy> Typed2718 for Receipt<T> {
    fn ty(&self) -> u8 {
        self.tx_type.ty()
    }
}

impl<T: TxTy> IsTyped2718 for Receipt<T> {
    fn is_type(type_id: u8) -> bool {
        <TxType as IsTyped2718>::is_type(type_id)
    }
}

impl<T: TxTy> InMemorySize for Receipt<T> {
    fn size(&self) -> usize {
        self.tx_type.size() +
            core::mem::size_of::<bool>() +
            core::mem::size_of::<u64>() +
            self.logs.capacity() * core::mem::size_of::<Log>()
    }
}

impl<T> From<alloy_consensus::ReceiptEnvelope<T>> for Receipt<TxType>
where
    T: Into<Log>,
{
    fn from(value: alloy_consensus::ReceiptEnvelope<T>) -> Self {
        let value = value.into_primitives_receipt();
        Self {
            tx_type: value.tx_type(),
            success: value.is_success(),
            cumulative_gas_used: value.cumulative_gas_used(),
            // TODO: remove after <https://github.com/alloy-rs/alloy/pull/2533>
            logs: value.logs().to_vec(),
        }
    }
}

impl<T> From<Receipt<T>> for alloy_consensus::Receipt<Log> {
    fn from(value: Receipt<T>) -> Self {
        Self {
            status: value.success.into(),
            cumulative_gas_used: value.cumulative_gas_used,
            logs: value.logs,
        }
    }
}

impl From<Receipt<TxType>> for alloy_consensus::ReceiptEnvelope<Log> {
    fn from(value: Receipt<TxType>) -> Self {
        let tx_type = value.tx_type;
        let receipt = value.into_with_bloom().map_receipt(Into::into);
        match tx_type {
            TxType::Legacy => Self::Legacy(receipt),
            TxType::Eip2930 => Self::Eip2930(receipt),
            TxType::Eip1559 => Self::Eip1559(receipt),
            TxType::Eip4844 => Self::Eip4844(receipt),
            TxType::Eip7702 => Self::Eip7702(receipt),
        }
    }
}

#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub(super) mod serde_bincode_compat {
    use alloc::{borrow::Cow, vec::Vec};
    use alloy_consensus::TxType;
    use alloy_eips::eip2718::Eip2718Error;
    use alloy_primitives::{Log, U8};
    use core::fmt::Debug;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::Receipt`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use alloy_consensus::TxType;
    /// use reth_ethereum_primitives::{serde_bincode_compat, Receipt};
    /// use serde::{de::DeserializeOwned, Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::Receipt<'_>")]
    ///     receipt: Receipt<TxType>,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(bound(deserialize = "T: TryFrom<u8, Error = Eip2718Error>"))]
    pub struct Receipt<'a, T = TxType> {
        /// Receipt type.
        #[serde(deserialize_with = "deserde_txtype")]
        pub tx_type: T,
        /// If transaction is executed successfully.
        ///
        /// This is the `statusCode`
        pub success: bool,
        /// Gas used
        pub cumulative_gas_used: u64,
        /// Log send from contracts.
        pub logs: Cow<'a, Vec<Log>>,
    }

    /// Ensures that txtype is deserialized symmetrically as U8
    fn deserde_txtype<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: TryFrom<u8, Error = Eip2718Error>,
    {
        U8::deserialize(deserializer)?.to::<u8>().try_into().map_err(serde::de::Error::custom)
    }

    impl<'a, T: Copy> From<&'a super::Receipt<T>> for Receipt<'a, T> {
        fn from(value: &'a super::Receipt<T>) -> Self {
            Self {
                tx_type: value.tx_type,
                success: value.success,
                cumulative_gas_used: value.cumulative_gas_used,
                logs: Cow::Borrowed(&value.logs),
            }
        }
    }

    impl<'a, T> From<Receipt<'a, T>> for super::Receipt<T> {
        fn from(value: Receipt<'a, T>) -> Self {
            Self {
                tx_type: value.tx_type,
                success: value.success,
                cumulative_gas_used: value.cumulative_gas_used,
                logs: value.logs.into_owned(),
            }
        }
    }

    impl<T: Copy + Serialize> SerializeAs<super::Receipt<T>> for Receipt<'_, T> {
        fn serialize_as<S>(source: &super::Receipt<T>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Receipt::<'_>::from(source).serialize(serializer)
        }
    }

    impl<'de, T: TryFrom<u8, Error = Eip2718Error>> DeserializeAs<'de, super::Receipt<T>>
        for Receipt<'de, T>
    {
        fn deserialize_as<D>(deserializer: D) -> Result<super::Receipt<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            Receipt::<'_, T>::deserialize(deserializer).map(Into::into)
        }
    }

    impl<T> reth_primitives_traits::serde_bincode_compat::SerdeBincodeCompat for super::Receipt<T>
    where
        T: Copy + Serialize + TryFrom<u8, Error = Eip2718Error> + Debug + 'static,
    {
        type BincodeRepr<'a> = Receipt<'a, T>;

        fn as_repr(&self) -> Self::BincodeRepr<'_> {
            self.into()
        }

        fn from_repr(repr: Self::BincodeRepr<'_>) -> Self {
            repr.into()
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::{receipt::serde_bincode_compat, Receipt};
        use alloy_consensus::TxType;
        use arbitrary::Arbitrary;
        use rand::Rng;
        use serde_with::serde_as;

        #[test]
        fn test_receipt_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq)]
            #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::Receipt<'_, TxType>")]
                receipt: Receipt<TxType>,
            }

            let mut bytes = [0u8; 1024];
            rand::rng().fill(bytes.as_mut_slice());
            let data = Data {
                receipt: Receipt::arbitrary(&mut arbitrary::Unstructured::new(&bytes)).unwrap(),
            };
            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(feature = "reth-codec")]
mod compact {
    use super::*;
    use reth_codecs::{
        Compact,
        __private::{modular_bitfield::prelude::*, Buf},
    };

    impl Receipt {
        #[doc = "Used bytes by [`ReceiptFlags`]"]
        pub const fn bitflag_encoded_bytes() -> usize {
            1u8 as usize
        }
        #[doc = "Unused bits for new fields by [`ReceiptFlags`]"]
        pub const fn bitflag_unused_bits() -> usize {
            0u8 as usize
        }
    }

    #[allow(non_snake_case, unused_parens)]
    mod flags {
        use super::*;

        #[doc = "Fieldset that facilitates compacting the parent type. Used bytes: 1 | Unused bits: 0"]
        #[bitfield]
        #[derive(Clone, Copy, Debug, Default)]
        pub struct ReceiptFlags {
            pub tx_type_len: B2,
            pub success_len: B1,
            pub cumulative_gas_used_len: B4,
            pub __zstd: B1,
        }

        impl ReceiptFlags {
            #[doc = r" Deserializes this fieldset and returns it, alongside the original slice in an advanced position."]
            pub fn from(mut buf: &[u8]) -> (Self, &[u8]) {
                (Self::from_bytes([buf.get_u8()]), buf)
            }
        }
    }

    pub use flags::ReceiptFlags;

    impl<T: Compact> Compact for Receipt<T> {
        fn to_compact<B>(&self, buf: &mut B) -> usize
        where
            B: reth_codecs::__private::bytes::BufMut + AsMut<[u8]>,
        {
            let mut flags = ReceiptFlags::default();
            let mut total_length = 0;
            let mut buffer = reth_codecs::__private::bytes::BytesMut::new();

            let tx_type_len = self.tx_type.to_compact(&mut buffer);
            flags.set_tx_type_len(tx_type_len as u8);
            let success_len = self.success.to_compact(&mut buffer);
            flags.set_success_len(success_len as u8);
            let cumulative_gas_used_len = self.cumulative_gas_used.to_compact(&mut buffer);
            flags.set_cumulative_gas_used_len(cumulative_gas_used_len as u8);
            self.logs.to_compact(&mut buffer);

            let zstd = buffer.len() > 7;
            if zstd {
                flags.set___zstd(1);
            }

            let flags = flags.into_bytes();
            total_length += flags.len() + buffer.len();
            buf.put_slice(&flags);
            if zstd {
                reth_zstd_compressors::RECEIPT_COMPRESSOR.with(|compressor| {
                    let compressed =
                        compressor.borrow_mut().compress(&buffer).expect("Failed to compress.");
                    buf.put(compressed.as_slice());
                });
            } else {
                buf.put(buffer);
            }
            total_length
        }

        fn from_compact(buf: &[u8], _len: usize) -> (Self, &[u8]) {
            let (flags, mut buf) = ReceiptFlags::from(buf);
            if flags.__zstd() != 0 {
                reth_zstd_compressors::RECEIPT_DECOMPRESSOR.with(|decompressor| {
                    let decompressor = &mut decompressor.borrow_mut();
                    let decompressed = decompressor.decompress(buf);
                    let original_buf = buf;
                    let mut buf: &[u8] = decompressed;
                    let (tx_type, new_buf) = T::from_compact(buf, flags.tx_type_len() as usize);
                    buf = new_buf;
                    let (success, new_buf) = bool::from_compact(buf, flags.success_len() as usize);
                    buf = new_buf;
                    let (cumulative_gas_used, new_buf) =
                        u64::from_compact(buf, flags.cumulative_gas_used_len() as usize);
                    buf = new_buf;
                    let (logs, _) = Vec::from_compact(buf, buf.len());
                    (Self { tx_type, success, cumulative_gas_used, logs }, original_buf)
                })
            } else {
                let (tx_type, new_buf) = T::from_compact(buf, flags.tx_type_len() as usize);
                buf = new_buf;
                let (success, new_buf) = bool::from_compact(buf, flags.success_len() as usize);
                buf = new_buf;
                let (cumulative_gas_used, new_buf) =
                    u64::from_compact(buf, flags.cumulative_gas_used_len() as usize);
                buf = new_buf;
                let (logs, new_buf) = Vec::from_compact(buf, buf.len());
                buf = new_buf;
                let obj = Self { tx_type, success, cumulative_gas_used, logs };
                (obj, buf)
            }
        }
    }
}

#[cfg(feature = "reth-codec")]
pub use compact::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TransactionSigned;
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{
        address, b256, bloom, bytes, hex_literal::hex, Address, Bytes, Log, LogData,
    };
    use alloy_rlp::Decodable;
    use reth_codecs::Compact;
    use reth_primitives_traits::proofs::{
        calculate_receipt_root, calculate_transaction_root, calculate_withdrawals_root,
    };

    /// Ethereum full block.
    ///
    /// Withdrawals can be optionally included at the end of the RLP encoded message.
    pub(crate) type Block<T = TransactionSigned> = alloy_consensus::Block<T>;

    #[test]
    fn test_decode_receipt() {
        reth_codecs::test_utils::test_decode::<Receipt<TxType>>(&hex!(
            "c428b52ffd23fc42696156b10200f034792b6a94c3850215c2fef7aea361a0c31b79d9a32652eefc0d4e2e730036061cff7344b6fc6132b50cda0ed810a991ae58ef013150c12b2522533cb3b3a8b19b7786a8b5ff1d3cdc84225e22b02def168c8858df"
        ));
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn encode_legacy_receipt() {
        let expected = hex!(
            "f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff"
        );

        let mut data = Vec::with_capacity(expected.length());
        let receipt = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 0x1u64,
                logs: vec![Log::new_unchecked(
                    address!("0x0000000000000000000000000000000000000011"),
                    vec![
                        b256!("0x000000000000000000000000000000000000000000000000000000000000dead"),
                        b256!("0x000000000000000000000000000000000000000000000000000000000000beef"),
                    ],
                    bytes!("0100ff"),
                )],
                success: false,
            },
            logs_bloom: [0; 256].into(),
        };

        receipt.encode(&mut data);

        // check that the rlp length equals the length of the expected rlp
        assert_eq!(receipt.length(), expected.len());
        assert_eq!(data, expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn decode_legacy_receipt() {
        let data = hex!(
            "f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff"
        );

        // EIP658Receipt
        let expected = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 0x1u64,
                logs: vec![Log::new_unchecked(
                    address!("0x0000000000000000000000000000000000000011"),
                    vec![
                        b256!("0x000000000000000000000000000000000000000000000000000000000000dead"),
                        b256!("0x000000000000000000000000000000000000000000000000000000000000beef"),
                    ],
                    bytes!("0100ff"),
                )],
                success: false,
            },
            logs_bloom: [0; 256].into(),
        };

        let receipt = ReceiptWithBloom::decode(&mut &data[..]).unwrap();
        assert_eq!(receipt, expected);
    }

    #[test]
    fn gigantic_receipt() {
        let receipt = Receipt {
            cumulative_gas_used: 16747627,
            success: true,
            tx_type: TxType::Legacy,
            logs: vec![
                Log::new_unchecked(
                    address!("0x4bf56695415f725e43c3e04354b604bcfb6dfb6e"),
                    vec![b256!(
                        "0xc69dc3d7ebff79e41f525be431d5cd3cc08f80eaf0f7819054a726eeb7086eb9"
                    )],
                    Bytes::from(vec![1; 0xffffff]),
                ),
                Log::new_unchecked(
                    address!("0xfaca325c86bf9c2d5b413cd7b90b209be92229c2"),
                    vec![b256!(
                        "0x8cca58667b1e9ffa004720ac99a3d61a138181963b294d270d91c53d36402ae2"
                    )],
                    Bytes::from(vec![1; 0xffffff]),
                ),
            ],
        };

        let mut data = vec![];
        receipt.to_compact(&mut data);
        let (decoded, _) = Receipt::<TxType>::from_compact(&data[..], data.len());
        assert_eq!(decoded, receipt);
    }

    #[test]
    fn test_encode_2718_length() {
        let receipt = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Eip1559,
                success: true,
                cumulative_gas_used: 21000,
                logs: vec![],
            },
            logs_bloom: Bloom::default(),
        };

        let encoded = receipt.encoded_2718();
        assert_eq!(
            encoded.len(),
            receipt.encode_2718_len(),
            "Encoded length should match the actual encoded data length"
        );

        // Test for legacy receipt as well
        let legacy_receipt = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Legacy,
                success: true,
                cumulative_gas_used: 21000,
                logs: vec![],
            },
            logs_bloom: Bloom::default(),
        };

        let legacy_encoded = legacy_receipt.encoded_2718();
        assert_eq!(
            legacy_encoded.len(),
            legacy_receipt.encode_2718_len(),
            "Encoded length for legacy receipt should match the actual encoded data length"
        );
    }

    #[test]
    fn check_transaction_root() {
        let data = &hex!(
            "f90262f901f9a092230ce5476ae868e98c7979cfc165a93f8b6ad1922acf2df62e340916efd49da01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa02307107a867056ca33b5087e77c4174f47625e48fb49f1c70ced34890ddd88f3a08151d548273f6683169524b66ca9fe338b9ce42bc3540046c828fd939ae23bcba0c598f69a5674cae9337261b669970e24abc0b46e6d284372a239ec8ccbf20b0ab901000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000083020000018502540be40082a8618203e800a00000000000000000000000000000000000000000000000000000000000000000880000000000000000f863f861800a8405f5e10094100000000000000000000000000000000000000080801ba07e09e26678ed4fac08a249ebe8ed680bf9051a5e14ad223e4b2b9d26e0208f37a05f6e3f188e3e6eab7d7d3b6568f5eac7d687b08d307d3154ccd8c87b4630509bc0"
        );
        let block_rlp = &mut data.as_slice();
        let block: Block = Block::decode(block_rlp).unwrap();

        let tx_root = calculate_transaction_root(&block.body.transactions);
        assert_eq!(block.transactions_root, tx_root, "Must be the same");
    }

    #[test]
    fn check_withdrawals_root() {
        // Single withdrawal, amount 0
        // https://github.com/ethereum/tests/blob/9760400e667eba241265016b02644ef62ab55de2/BlockchainTests/EIPTests/bc4895-withdrawals/amountIs0.json
        let data = &hex!(
            "f90238f90219a0151934ad9b654c50197f37018ee5ee9bb922dec0a1b5e24a6d679cb111cdb107a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa0046119afb1ab36aaa8f66088677ed96cd62762f6d3e65642898e189fbe702d51a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff8082079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a048a703da164234812273ea083e4ec3d09d028300cd325b46a6a75402e5a7ab95c0c0d9d8808094c94f5374fce5edbc8e2a8697c15331677e6ebf0b80"
        );
        let block: Block = Block::decode(&mut data.as_slice()).unwrap();
        assert!(block.body.withdrawals.is_some());
        let withdrawals = block.body.withdrawals.as_ref().unwrap();
        assert_eq!(withdrawals.len(), 1);
        let withdrawals_root = calculate_withdrawals_root(withdrawals);
        assert_eq!(block.withdrawals_root, Some(withdrawals_root));

        // 4 withdrawals, identical indices
        // https://github.com/ethereum/tests/blob/9760400e667eba241265016b02644ef62ab55de2/BlockchainTests/EIPTests/bc4895-withdrawals/twoIdenticalIndex.json
        let data = &hex!(
            "f9028cf90219a0151934ad9b654c50197f37018ee5ee9bb922dec0a1b5e24a6d679cb111cdb107a01dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347942adc25665018aa1fe0e6bc666dac8fc2697ff9baa0ccf7b62d616c2ad7af862d67b9dcd2119a90cebbff8c3cd1e5d7fc99f8755774a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008001887fffffffffffffff8082079e42a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b42188000000000000000009a0a95b9a7b58a6b3cb4001eb0be67951c5517141cb0183a255b5cae027a7b10b36c0c0f86cda808094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da028094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da018094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710da028094c94f5374fce5edbc8e2a8697c15331677e6ebf0b822710"
        );
        let block: Block = Block::decode(&mut data.as_slice()).unwrap();
        assert!(block.body.withdrawals.is_some());
        let withdrawals = block.body.withdrawals.as_ref().unwrap();
        assert_eq!(withdrawals.len(), 4);
        let withdrawals_root = calculate_withdrawals_root(withdrawals);
        assert_eq!(block.withdrawals_root, Some(withdrawals_root));
    }
    #[test]
    fn check_receipt_root_optimism() {
        use alloy_consensus::ReceiptWithBloom;

        let logs = vec![Log {
            address: Address::ZERO,
            data: LogData::new_unchecked(vec![], Default::default()),
        }];
        let bloom = bloom!(
            "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001"
        );
        let receipt = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Eip2930,
                success: true,
                cumulative_gas_used: 102068,
                logs,
            },
            logs_bloom: bloom,
        };
        let receipt = vec![receipt];
        let root = calculate_receipt_root(&receipt);
        assert_eq!(
            root,
            b256!("0xfe70ae4a136d98944951b2123859698d59ad251a381abc9960fa81cae3d0d4a0")
        );
    }
}
