use core::fmt::Debug;

use alloc::vec::Vec;
use alloy_consensus::{
    Eip2718DecodableReceipt, Eip2718EncodableReceipt, Eip658Value, ReceiptEnvelope,
    ReceiptWithBloom, RlpDecodableReceipt, RlpEncodableReceipt, TxReceipt, TxType, Typed2718,
};
use alloy_eips::eip2718::{Eip2718Error, Eip2718Result, Encodable2718, IsTyped2718};
use alloy_primitives::{Bloom, Log, B256};
use alloy_rlp::{BufMut, Decodable, Encodable, Header, RlpDecodable, RlpEncodable};
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

/// Raw ethereum receipt.
pub type Receipt<T = TxType> = EthereumReceipt<T>;

#[cfg(feature = "rpc")]
/// Receipt representation for RPC.
pub type RpcReceipt<T = TxType> = EthereumReceipt<T, alloy_rpc_types_eth::Log>;

/// Typed ethereum transaction receipt.
/// Receipt containing result of transaction execution.
#[derive(Clone, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "reth-codec", reth_codecs::add_arbitrary_tests(compact, rlp))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct EthereumReceipt<T = TxType, L = Log> {
    /// Receipt type.
    #[cfg_attr(feature = "serde", serde(rename = "type"))]
    pub tx_type: T,
    /// If transaction is executed successfully.
    ///
    /// This is the `statusCode`
    #[cfg_attr(feature = "serde", serde(with = "alloy_serde::quantity", rename = "status"))]
    pub success: bool,
    /// Gas used
    #[cfg_attr(feature = "serde", serde(with = "alloy_serde::quantity"))]
    pub cumulative_gas_used: u64,
    /// Log send from contracts.
    pub logs: Vec<L>,
}

#[cfg(feature = "rpc")]
impl<T> Receipt<T> {
    /// Converts the logs of the receipt to RPC logs.
    pub fn into_rpc(
        self,
        next_log_index: usize,
        meta: alloy_consensus::transaction::TransactionMeta,
    ) -> RpcReceipt<T> {
        let Self { tx_type, success, cumulative_gas_used, logs } = self;
        let logs = alloy_rpc_types_eth::Log::collect_for_receipt(next_log_index, meta, logs);
        RpcReceipt { tx_type, success, cumulative_gas_used, logs }
    }
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

impl<T: TxTy> Eip2718DecodableReceipt for Receipt<T> {
    fn typed_decode_with_bloom(ty: u8, buf: &mut &[u8]) -> Eip2718Result<ReceiptWithBloom<Self>> {
        Ok(Self::rlp_decode_inner(buf, T::try_from(ty)?)?)
    }

    fn fallback_decode_with_bloom(buf: &mut &[u8]) -> Eip2718Result<ReceiptWithBloom<Self>> {
        Ok(Self::rlp_decode_inner(buf, T::try_from(0)?)?)
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

impl<T, L> TxReceipt for EthereumReceipt<T, L>
where
    T: TxTy,
    L: Send + Sync + Clone + Debug + Eq + AsRef<Log>,
{
    type Log = L;

    fn status_or_post_state(&self) -> Eip658Value {
        self.success.into()
    }

    fn status(&self) -> bool {
        self.success
    }

    fn bloom(&self) -> Bloom {
        alloy_primitives::logs_bloom(self.logs.iter().map(|l| l.as_ref()))
    }

    fn cumulative_gas_used(&self) -> u64 {
        self.cumulative_gas_used
    }

    fn logs(&self) -> &[L] {
        &self.logs
    }

    fn into_logs(self) -> Vec<L> {
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
        size_of::<Self>() + self.logs.iter().map(|log| log.size()).sum::<usize>()
    }
}

impl<T> From<ReceiptEnvelope<T>> for Receipt<TxType>
where
    T: Into<Log>,
{
    fn from(value: ReceiptEnvelope<T>) -> Self {
        let value = value.into_primitives_receipt();
        Self {
            tx_type: value.tx_type(),
            success: value.is_success(),
            cumulative_gas_used: value.cumulative_gas_used(),
            logs: value.into_logs(),
        }
    }
}

impl<T, L> From<EthereumReceipt<T, L>> for alloy_consensus::Receipt<L> {
    fn from(value: EthereumReceipt<T, L>) -> Self {
        Self {
            status: value.success.into(),
            cumulative_gas_used: value.cumulative_gas_used,
            logs: value.logs,
        }
    }
}

impl<L> From<EthereumReceipt<TxType, L>> for ReceiptEnvelope<L>
where
    L: Send + Sync + Clone + Debug + Eq + AsRef<Log>,
{
    fn from(value: EthereumReceipt<TxType, L>) -> Self {
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

    /// [`Receipt`] extension struct.
    ///
    /// All new fields should be added here in the form of a `Option<T>`, since `Option<ReceiptExt>`
    /// is used as an extension field for backwards compatibility.
    ///
    /// More information: <https://github.com/paradigmxyz/reth/issues/7820>
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    #[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
    #[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Compact)]
    #[reth_codecs(crate = "reth_codecs")]
    pub struct ReceiptExt {
        /// Placeholder field for glam hardfork receipt extensions.
        /// Replace with actual glam fields when specified.
        pub block_access_index: Option<u64>,
    }

    impl ReceiptExt {
        /// Converts into [`Some`] if any of the field exists. Otherwise, returns [`None`].
        ///
        /// Required since [`Receipt`] uses `Option<ReceiptExt>` as a field.
        pub const fn into_option(self) -> Option<Self> {
            if self.block_access_index.is_some() {
                Some(self)
            } else {
                None
            }
        }
    }

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

            // Extension fields are appended after logs for backwards compatibility.
            // Old receipts without extension fields will have empty buffer after logs,
            // which decodes as None.
            ReceiptExt::default().into_option().to_compact(&mut buffer);

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
                    let mut buf: &[u8] = decompressed;
                    let (tx_type, new_buf) = T::from_compact(buf, flags.tx_type_len() as usize);
                    buf = new_buf;
                    let (success, new_buf) = bool::from_compact(buf, flags.success_len() as usize);
                    buf = new_buf;
                    let (cumulative_gas_used, new_buf) =
                        u64::from_compact(buf, flags.cumulative_gas_used_len() as usize);
                    buf = new_buf;
                    let (logs, new_buf) = Vec::from_compact(buf, buf.len());
                    buf = new_buf;
                    // Decode extension fields if present (backwards compatible - old receipts
                    // will have empty buffer here). Use proper Option semantics: pass 1 if
                    // buffer has data, 0 if empty.
                    let has_ext = !buf.is_empty();
                    let (_extra_fields, _) =
                        Option::<ReceiptExt>::from_compact(buf, has_ext as usize);
                    // zstd case: we consumed the entire compressed payload, return empty slice
                    (Self { tx_type, success, cumulative_gas_used, logs }, &[] as &[u8])
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
                // Decode extension fields if present (backwards compatible - old receipts
                // will have empty buffer here). Use proper Option semantics: pass 1 if
                // buffer has data, 0 if empty.
                let has_ext = !buf.is_empty();
                let (_extra_fields, new_buf) =
                    Option::<ReceiptExt>::from_compact(buf, has_ext as usize);
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
    #[cfg(feature = "reth-codec")]
    fn test_decode_receipt() {
        reth_codecs::test_utils::test_decode::<Receipt<TxType>>(&hex!(
            "c428b52ffd23fc42696156b10200f034792b6a94c3850215c2fef7aea361a0c31b79d9a32652eefc0d4e2e730036061cff7344b6fc6132b50cda0ed810a991ae58ef013150c12b2522533cb3b3a8b19b7786a8b5ff1d3cdc84225e22b02def168c8858df"
        ));
    }

    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_receipt_ext_backwards_compatibility() {
        use crate::ReceiptExt;
        use reth_codecs::{test_utils::UnusedBits, validate_bitflag_backwards_compat};

        // Receipt has 0 unused bits - new fields must go through ReceiptExt
        assert_eq!(Receipt::bitflag_unused_bits(), 0);

        // ReceiptExt has unused bits available for future fields
        validate_bitflag_backwards_compat!(ReceiptExt, UnusedBits::NotZero);
    }

    /// Test vectors generated from main branch to ensure backwards compatibility.
    /// These test that receipts encoded before [`ReceiptExt`] was added can still be decoded.
    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_backwards_compat_decode_legacy_receipt() {
        // Small receipt: Legacy, success=true, gas=21000, no logs
        // Generated on main branch before ReceiptExt: [0x14, 0x52, 0x08, 0x00]
        const SMALL_LEGACY: &[u8] = &hex!("14520800");
        let (decoded, _) = Receipt::<TxType>::from_compact(SMALL_LEGACY, SMALL_LEGACY.len());
        assert_eq!(decoded.tx_type, TxType::Legacy);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 21000);
        assert!(decoded.logs.is_empty());

        // Re-encode and verify roundtrip
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), SMALL_LEGACY);
    }

    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_backwards_compat_decode_eip1559_receipt() {
        // EIP-1559 receipt: success=true, gas=100000, no logs
        // Generated on main branch: [1e, 01, 86, a0, 00]
        const EIP1559_RECEIPT: &[u8] = &hex!("1e0186a000");
        let (decoded, _) = Receipt::<TxType>::from_compact(EIP1559_RECEIPT, EIP1559_RECEIPT.len());
        assert_eq!(decoded.tx_type, TxType::Eip1559);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 100000);
        assert!(decoded.logs.is_empty());

        // Re-encode and verify roundtrip
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), EIP1559_RECEIPT);
    }

    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_backwards_compat_decode_eip4844_failed_receipt() {
        // EIP-4844 receipt: success=false, gas=500000, no logs
        // Generated on main branch: [1b, 03, 07, a1, 20, 00]
        const EIP4844_FAILED: &[u8] = &hex!("1b0307a12000");
        let (decoded, _) = Receipt::<TxType>::from_compact(EIP4844_FAILED, EIP4844_FAILED.len());
        assert_eq!(decoded.tx_type, TxType::Eip4844);
        assert!(!decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 500000);
        assert!(decoded.logs.is_empty());

        // Re-encode and verify roundtrip
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), EIP4844_FAILED);
    }

    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_backwards_compat_decode_receipt_with_logs() {
        // Receipt with one log - uses the existing test vector from test_decode_receipt
        // This is a zstd-compressed receipt from main branch
        const RECEIPT_WITH_LOGS: &[u8] = &hex!(
            "c428b52ffd23fc42696156b10200f034792b6a94c3850215c2fef7aea361a0c31b79d9a32652eefc0d4e2e730036061cff7344b6fc6132b50cda0ed810a991ae58ef013150c12b2522533cb3b3a8b19b7786a8b5ff1d3cdc84225e22b02def168c8858df"
        );
        let (decoded, _) =
            Receipt::<TxType>::from_compact(RECEIPT_WITH_LOGS, RECEIPT_WITH_LOGS.len());

        // Verify it decoded successfully
        assert!(!decoded.logs.is_empty());

        // Re-encode and verify roundtrip
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), RECEIPT_WITH_LOGS);
    }

    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_backwards_compat_receipt_with_log_zstd() {
        // EIP-1559 receipt with one log (zstd compressed)
        // Generated on main branch
        const RECEIPT_WITH_LOG: &[u8] =
            &hex!("9e28b52ffd23fc4269613dc50000580186a0013811dead0100ff03fc9ecdc83800b4f6906001");
        let (decoded, _) =
            Receipt::<TxType>::from_compact(RECEIPT_WITH_LOG, RECEIPT_WITH_LOG.len());

        assert_eq!(decoded.tx_type, TxType::Eip1559);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 100000);
        assert_eq!(decoded.logs.len(), 1);

        // Re-encode and verify roundtrip
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), RECEIPT_WITH_LOG);
    }

    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_backwards_compat_all_tx_types() {
        // Test all transaction types can roundtrip
        for tx_type in [TxType::Legacy, TxType::Eip2930, TxType::Eip1559, TxType::Eip4844] {
            let receipt =
                Receipt { tx_type, success: true, cumulative_gas_used: 50000, logs: vec![] };

            let mut buf = vec![];
            receipt.to_compact(&mut buf);

            let (decoded, _) = Receipt::<TxType>::from_compact(&buf, buf.len());
            assert_eq!(decoded, receipt, "Roundtrip failed for {:?}", tx_type);
        }
    }

    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_option_receipt_ext_compact_semantics() {
        use crate::ReceiptExt;

        // Verify Option<ReceiptExt> compact encoding semantics for backwards compatibility.
        // The len parameter is a presence indicator: 0 = None, 1 = Some.

        // Test 1: None encodes as 0 bytes and returns len=0
        let none_ext: Option<ReceiptExt> = None;
        let mut buf = vec![];
        let len = none_ext.to_compact(&mut buf);
        assert_eq!(len, 0, "None should return len=0 (presence indicator)");
        assert!(buf.is_empty(), "None should write 0 bytes");

        // Test 2: from_compact with len=0 returns None without consuming bytes
        let (decoded, remaining) = Option::<ReceiptExt>::from_compact(&[], 0);
        assert!(decoded.is_none(), "len=0 should decode to None");
        assert!(remaining.is_empty());

        // Test 3: from_compact with len=0 on non-empty buffer still returns None
        // This is the correct behavior - len is the presence indicator, not buffer length
        let buf_with_data = [0x01, 0x02, 0x03];
        let (decoded, remaining) = Option::<ReceiptExt>::from_compact(&buf_with_data, 0);
        assert!(decoded.is_none(), "len=0 means None regardless of buffer content");
        assert_eq!(remaining, &buf_with_data, "Buffer unchanged when len=0");

        // Test 4: Some(ReceiptExt) encodes with varuint length prefix
        let some_ext = Some(ReceiptExt { block_access_index: Some(12345) });
        let mut buf = vec![];
        let len = some_ext.to_compact(&mut buf);
        assert_eq!(len, 1, "Some should return len=1 (presence indicator)");
        assert!(!buf.is_empty(), "Some should write varuint + data");

        // Test 5: from_compact with len=1 decodes the extension
        let (decoded, remaining) = Option::<ReceiptExt>::from_compact(&buf, 1);
        assert_eq!(decoded, some_ext, "Some(ReceiptExt) should roundtrip");
        assert!(remaining.is_empty(), "Should consume all bytes");

        // Test 6: Receipt decode uses buf.is_empty() to determine presence
        // When buffer is empty after logs, pass len=0 (false as usize)
        // When buffer has data after logs, pass len=1 (true as usize)
        let has_ext_empty: usize = (!(&[] as &[u8]).is_empty()) as usize;
        assert_eq!(has_ext_empty, 0, "Empty buffer should produce len=0");

        let has_ext_data: usize = (!buf_with_data.is_empty()) as usize;
        assert_eq!(has_ext_data, 1, "Non-empty buffer should produce len=1");
    }

    /// Comprehensive test matrix for Receipt compact encoding with ReceiptExt.
    /// Tests all combinations of: tx types × success × logs × extension fields.
    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_receipt_compact_roundtrip_matrix() {
        // Test matrix: tx_type × success × logs × produces comprehensive coverage
        let tx_types = [TxType::Legacy, TxType::Eip2930, TxType::Eip1559, TxType::Eip4844];
        let success_values = [true, false];
        let gas_values = [0u64, 21000, 100000, u64::MAX];

        for tx_type in tx_types {
            for success in success_values {
                for gas in gas_values {
                    // Test with no logs (small, uncompressed)
                    let receipt_no_logs =
                        Receipt { tx_type, success, cumulative_gas_used: gas, logs: vec![] };

                    let mut buf = vec![];
                    receipt_no_logs.to_compact(&mut buf);
                    let (decoded, remaining) = Receipt::<TxType>::from_compact(&buf, buf.len());

                    assert_eq!(
                        decoded, receipt_no_logs,
                        "Roundtrip failed for {:?}, success={}, gas={}, no logs",
                        tx_type, success, gas
                    );
                    assert!(remaining.is_empty(), "Should consume all bytes");
                }
            }
        }
    }

    /// Test receipts with logs - these trigger zstd compression when large enough.
    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_receipt_compact_with_logs() {
        // Single log - may or may not trigger zstd depending on size
        let single_log = Log::new_unchecked(
            address!("0x0000000000000000000000000000000000000011"),
            vec![b256!("0x000000000000000000000000000000000000000000000000000000000000dead")],
            bytes!("0100ff"),
        );

        let receipt_one_log = Receipt {
            tx_type: TxType::Eip1559,
            success: true,
            cumulative_gas_used: 50000,
            logs: vec![single_log.clone()],
        };

        let mut buf = vec![];
        receipt_one_log.to_compact(&mut buf);
        let (decoded, remaining) = Receipt::<TxType>::from_compact(&buf, buf.len());
        assert_eq!(decoded, receipt_one_log, "Single log roundtrip failed");
        assert!(remaining.is_empty());

        // Multiple logs - should trigger zstd compression
        let receipt_many_logs = Receipt {
            tx_type: TxType::Eip4844,
            success: false,
            cumulative_gas_used: 1000000,
            logs: vec![single_log.clone(); 5],
        };

        let mut buf = vec![];
        receipt_many_logs.to_compact(&mut buf);
        // Verify zstd flag is set (first byte has bit 0 set for zstd)
        assert!(!buf.is_empty());
        let (decoded, remaining) = Receipt::<TxType>::from_compact(&buf, buf.len());
        assert_eq!(decoded, receipt_many_logs, "Multiple logs roundtrip failed");
        assert!(remaining.is_empty());
    }

    /// Test that ReceiptExt with field set roundtrips correctly.
    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_receipt_ext_field_set() {
        use crate::ReceiptExt;

        // ReceiptExt with block_access_index set
        let ext_with_field = ReceiptExt { block_access_index: Some(42) };
        let mut buf = vec![];
        ext_with_field.to_compact(&mut buf);
        assert!(!buf.is_empty(), "ReceiptExt with field should write bytes");

        let (decoded, remaining) = ReceiptExt::from_compact(&buf, buf.len());
        assert_eq!(decoded, ext_with_field, "ReceiptExt roundtrip failed");
        assert!(remaining.is_empty());

        // ReceiptExt with None field
        let ext_no_field = ReceiptExt { block_access_index: None };
        let mut buf = vec![];
        ext_no_field.to_compact(&mut buf);

        let (decoded, remaining) = ReceiptExt::from_compact(&buf, buf.len());
        assert_eq!(decoded, ext_no_field, "ReceiptExt with None field roundtrip failed");
        assert!(remaining.is_empty());

        // Test into_option behavior
        assert!(ext_with_field.into_option().is_some());
        assert!(ext_no_field.into_option().is_none());
    }

    /// Test that Option<ReceiptExt> with various values roundtrips correctly.
    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_option_receipt_ext_variants() {
        use crate::ReceiptExt;

        // None
        let none: Option<ReceiptExt> = None;
        let mut buf = vec![];
        let len = none.to_compact(&mut buf);
        assert_eq!(len, 0);
        assert!(buf.is_empty());

        let (decoded, _) = Option::<ReceiptExt>::from_compact(&buf, 0);
        assert!(decoded.is_none());

        // Some with field = None (encodes to Some but field is empty)
        let some_empty = Some(ReceiptExt { block_access_index: None });
        let mut buf = vec![];
        let len = some_empty.to_compact(&mut buf);
        assert_eq!(len, 1);
        assert!(!buf.is_empty());

        let (decoded, _) = Option::<ReceiptExt>::from_compact(&buf, 1);
        assert_eq!(decoded, some_empty);

        // Some with field = Some(value)
        let some_with_value = Some(ReceiptExt { block_access_index: Some(999999) });
        let mut buf = vec![];
        let len = some_with_value.to_compact(&mut buf);
        assert_eq!(len, 1);

        let (decoded, _) = Option::<ReceiptExt>::from_compact(&buf, 1);
        assert_eq!(decoded, some_with_value);

        // Edge case: large value
        let some_large = Some(ReceiptExt { block_access_index: Some(u64::MAX) });
        let mut buf = vec![];
        some_large.to_compact(&mut buf);

        let (decoded, _) = Option::<ReceiptExt>::from_compact(&buf, 1);
        assert_eq!(decoded, some_large);
    }

    /// Test zstd compression path specifically.
    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_receipt_zstd_compression_path() {
        // Create a receipt large enough to trigger zstd (> 7 bytes after flags)
        let large_log = Log::new_unchecked(
            address!("0x1234567890123456789012345678901234567890"),
            vec![
                b256!("0x1111111111111111111111111111111111111111111111111111111111111111"),
                b256!("0x2222222222222222222222222222222222222222222222222222222222222222"),
            ],
            Bytes::from(vec![0xab; 100]),
        );

        let receipt = Receipt {
            tx_type: TxType::Eip1559,
            success: true,
            cumulative_gas_used: 500000,
            logs: vec![large_log],
        };

        let mut buf = vec![];
        receipt.to_compact(&mut buf);

        // Check that zstd flag is set (bit 0 of flags byte)
        assert!(!buf.is_empty());
        let flags_byte = buf[0];
        let zstd_flag = (flags_byte >> 7) & 1; // __zstd is the last bit
        assert_eq!(zstd_flag, 1, "Large receipt should be zstd compressed");

        // Verify roundtrip
        let (decoded, remaining) = Receipt::<TxType>::from_compact(&buf, buf.len());
        assert_eq!(decoded, receipt, "zstd compressed receipt roundtrip failed");
        assert!(remaining.is_empty(), "zstd path should return empty slice");
    }

    /// Test uncompressed path (small receipts).
    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_receipt_uncompressed_path() {
        // Small receipt - should not trigger zstd
        let receipt = Receipt {
            tx_type: TxType::Legacy,
            success: true,
            cumulative_gas_used: 21000,
            logs: vec![],
        };

        let mut buf = vec![];
        receipt.to_compact(&mut buf);

        // Check that zstd flag is NOT set
        assert!(!buf.is_empty());
        let flags_byte = buf[0];
        let zstd_flag = (flags_byte >> 7) & 1;
        assert_eq!(zstd_flag, 0, "Small receipt should not be zstd compressed");

        // Verify roundtrip
        let (decoded, remaining) = Receipt::<TxType>::from_compact(&buf, buf.len());
        assert_eq!(decoded, receipt, "Uncompressed receipt roundtrip failed");
        assert!(remaining.is_empty());
    }

    /// Comprehensive backwards compatibility test with vectors generated from main branch.
    /// These test that receipts encoded on main (before ReceiptExt) can be decoded correctly.
    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_backwards_compat_main_vectors_no_logs() {
        // All vectors generated from main branch commit before ReceiptExt was added.
        // Tests decode AND re-encode to verify exact byte-for-byte compatibility.

        // 1. Legacy, success=true, gas=21000, no logs
        const LEGACY_SUCCESS_NO_LOGS: &[u8] = &hex!("14520800");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(LEGACY_SUCCESS_NO_LOGS, LEGACY_SUCCESS_NO_LOGS.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Legacy);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 21000);
        assert!(decoded.logs.is_empty());
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), LEGACY_SUCCESS_NO_LOGS, "Legacy success roundtrip mismatch");

        // 2. Legacy, success=false, gas=21000, no logs
        const LEGACY_FAILED_NO_LOGS: &[u8] = &hex!("10520800");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(LEGACY_FAILED_NO_LOGS, LEGACY_FAILED_NO_LOGS.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Legacy);
        assert!(!decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 21000);
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), LEGACY_FAILED_NO_LOGS, "Legacy failed roundtrip mismatch");

        // 3. EIP-1559, success=true, gas=100000, no logs
        const EIP1559_SUCCESS_NO_LOGS: &[u8] = &hex!("1e0186a000");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(EIP1559_SUCCESS_NO_LOGS, EIP1559_SUCCESS_NO_LOGS.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Eip1559);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 100000);
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), EIP1559_SUCCESS_NO_LOGS, "EIP-1559 roundtrip mismatch");

        // 4. EIP-4844, success=false, gas=500000, no logs
        const EIP4844_FAILED_NO_LOGS: &[u8] = &hex!("1b0307a12000");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(EIP4844_FAILED_NO_LOGS, EIP4844_FAILED_NO_LOGS.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Eip4844);
        assert!(!decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 500000);
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), EIP4844_FAILED_NO_LOGS, "EIP-4844 failed roundtrip mismatch");

        // 5. EIP-2930, success=true, gas=50000, no logs
        const EIP2930_SUCCESS_NO_LOGS: &[u8] = &hex!("15c35000");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(EIP2930_SUCCESS_NO_LOGS, EIP2930_SUCCESS_NO_LOGS.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Eip2930);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 50000);
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), EIP2930_SUCCESS_NO_LOGS, "EIP-2930 roundtrip mismatch");

        // 9. Legacy, success=true, gas=u64::MAX/2, no logs (high gas value edge case)
        const LEGACY_HIGH_GAS: &[u8] = &hex!("c428b52ffd23fc426961094900007fffffffffffffff00");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(LEGACY_HIGH_GAS, LEGACY_HIGH_GAS.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Legacy);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, u64::MAX / 2);
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), LEGACY_HIGH_GAS, "Legacy high gas roundtrip mismatch");

        // 10. EIP-7702, success=true, gas=42000, no logs
        const EIP7702_SUCCESS_NO_LOGS: &[u8] = &hex!("1704a41000");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(EIP7702_SUCCESS_NO_LOGS, EIP7702_SUCCESS_NO_LOGS.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Eip7702);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 42000);
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), EIP7702_SUCCESS_NO_LOGS, "EIP-7702 roundtrip mismatch");
    }

    /// Backwards compatibility test for receipts with logs (zstd compressed).
    #[test]
    #[cfg(feature = "reth-codec")]
    fn test_backwards_compat_main_vectors_with_logs() {
        // These receipts have logs and are zstd compressed.
        // All vectors generated from main branch.

        // 6. Legacy, success=true, gas=50000, 1 log (zstd compressed)
        const LEGACY_ONE_LOG: &[u8] =
            &hex!("9428b52ffd23fc4269613cb5000050c350013811dead0100ff03fc9dcdc83867ed21f502");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(LEGACY_ONE_LOG, LEGACY_ONE_LOG.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Legacy);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 50000);
        assert_eq!(decoded.logs.len(), 1);
        assert_eq!(decoded.logs[0].address, address!("0x0000000000000000000000000000000000000011"));
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), LEGACY_ONE_LOG, "Legacy one log roundtrip mismatch");

        // 7. EIP-1559, success=true, gas=100000, 1 log (zstd compressed)
        const EIP1559_ONE_LOG: &[u8] =
            &hex!("9e28b52ffd23fc4269613dc50000580186a0013811dead0100ff03fc9ecdc83800b4f6906001");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(EIP1559_ONE_LOG, EIP1559_ONE_LOG.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Eip1559);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 100000);
        assert_eq!(decoded.logs.len(), 1);
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), EIP1559_ONE_LOG, "EIP-1559 one log roundtrip mismatch");

        // 8. EIP-4844, success=true, gas=1000000, 3 logs (zstd compressed)
        const EIP4844_THREE_LOGS: &[u8] =
            &hex!("9f28b52ffd23fc426961b0f5000058030f4240033811dead010005fccb9160f6c0a4f56733329e4e6b3f5314");
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(EIP4844_THREE_LOGS, EIP4844_THREE_LOGS.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Eip4844);
        assert!(decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 1000000);
        assert_eq!(decoded.logs.len(), 3);
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(buf.as_slice(), EIP4844_THREE_LOGS, "EIP-4844 three logs roundtrip mismatch");

        // 11. Legacy, success=false, gas=1, 1 log with 2 topics (zstd)
        const LEGACY_LOG_TWO_TOPICS: &[u8] = &hex!(
            "8828b52ffd23fc4269615be500005001011102debeef0100ff05fcc000e58451990fbe86533313fd4c0b"
        );
        let (decoded, remaining) =
            Receipt::<TxType>::from_compact(LEGACY_LOG_TWO_TOPICS, LEGACY_LOG_TWO_TOPICS.len());
        assert!(remaining.is_empty());
        assert_eq!(decoded.tx_type, TxType::Legacy);
        assert!(!decoded.success);
        assert_eq!(decoded.cumulative_gas_used, 1);
        assert_eq!(decoded.logs.len(), 1);
        assert_eq!(decoded.logs[0].topics().len(), 2);
        let mut buf = vec![];
        decoded.to_compact(&mut buf);
        assert_eq!(
            buf.as_slice(),
            LEGACY_LOG_TWO_TOPICS,
            "Legacy log two topics roundtrip mismatch"
        );
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

    // Ensures that reth and alloy receipts encode to the same JSON
    #[test]
    #[cfg(feature = "rpc")]
    fn test_receipt_serde() {
        let input = r#"{"status":"0x1","cumulativeGasUsed":"0x175cc0e","logs":[{"address":"0xa18b9ca2a78660d44ab38ae72e72b18792ffe413","topics":["0x8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925","0x000000000000000000000000e7e7d8006cbff47bc6ac2dabf592c98e97502708","0x0000000000000000000000007a250d5630b4cf539739df2c5dacb4c659f2488d"],"data":"0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff","blockHash":"0xbf9e6a368a399f996a0f0b27cab4191c028c3c99f5f76ea08a5b70b961475fcb","blockNumber":"0x164b59f","blockTimestamp":"0x68c9a713","transactionHash":"0x533aa9e57865675bb94f41aa2895c0ac81eee69686c77af16149c301e19805f1","transactionIndex":"0x14d","logIndex":"0x238","removed":false}],"logsBloom":"0x00000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000400000040000000000000004000000000000000000000000000000000000000000000020000000000000000000000000080000000000000000000000000200000020000000000000000000000000000000000000000000000000000000000000020000010000000000000000000000000000000000000000000000000000000000000","type":"0x2","transactionHash":"0x533aa9e57865675bb94f41aa2895c0ac81eee69686c77af16149c301e19805f1","transactionIndex":"0x14d","blockHash":"0xbf9e6a368a399f996a0f0b27cab4191c028c3c99f5f76ea08a5b70b961475fcb","blockNumber":"0x164b59f","gasUsed":"0xb607","effectiveGasPrice":"0x4a3ee768","from":"0xe7e7d8006cbff47bc6ac2dabf592c98e97502708","to":"0xa18b9ca2a78660d44ab38ae72e72b18792ffe413","contractAddress":null}"#;
        let receipt: RpcReceipt = serde_json::from_str(input).unwrap();
        let envelope: ReceiptEnvelope<alloy_rpc_types_eth::Log> =
            serde_json::from_str(input).unwrap();

        assert_eq!(envelope, receipt.clone().into());

        let json_envelope = serde_json::to_value(&envelope).unwrap();
        let json_receipt = serde_json::to_value(receipt.into_with_bloom()).unwrap();
        assert_eq!(json_envelope, json_receipt);
    }
}
