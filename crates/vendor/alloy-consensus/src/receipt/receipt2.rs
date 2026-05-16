use crate::{
    proofs::ordered_trie_root_with_encoder,
    receipt::{
        Eip2718DecodableReceipt, Eip2718EncodableReceipt, Eip658Value, RlpDecodableReceipt,
        RlpEncodableReceipt, TxReceipt,
    },
    InMemorySize, ReceiptEnvelope, ReceiptWithBloom, TxType,
};
use alloc::vec::Vec;
use alloy_eips::{
    eip2718::{Eip2718Error, Eip2718Result, Encodable2718, IsTyped2718},
    Typed2718,
};
use alloy_primitives::{Bloom, Log, B256};
use alloy_rlp::{BufMut, Decodable, Encodable, Header, RlpDecodable, RlpEncodable};
use core::fmt::Debug;

/// Helper trait alias with requirements for transaction type generic to be used within
/// [`EthereumReceipt`].
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
#[derive(Clone, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "arbitrary", derive(arbitrary::Arbitrary))]
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

impl<T, L> EthereumReceipt<T, L> {
    /// Converts the receipt's log type by applying a function to each log.
    ///
    /// Returns the receipt with the new log type.
    pub fn map_logs<U>(self, f: impl FnMut(L) -> U) -> EthereumReceipt<T, U> {
        let Self { tx_type, success, cumulative_gas_used, logs } = self;
        EthereumReceipt {
            tx_type,
            success,
            cumulative_gas_used,
            logs: logs.into_iter().map(f).collect(),
        }
    }
}

impl<T: TxTy> EthereumReceipt<T> {
    /// Returns length of RLP-encoded receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encoded_fields_length(&self, bloom: &Bloom) -> usize {
        self.success.length()
            + self.cumulative_gas_used.length()
            + bloom.length()
            + self.logs.length()
    }

    /// RLP-encodes receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encode_fields(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        self.success.encode(out);
        self.cumulative_gas_used.encode(out);
        bloom.encode(out);
        self.logs.encode(out);
    }

    /// Returns RLP header for this receipt encoding with the given [`Bloom`].
    pub fn rlp_header_inner(&self, bloom: &Bloom) -> Header {
        Header { list: true, payload_length: self.rlp_encoded_fields_length(bloom) }
    }

    /// RLP-decodes receipt and [`Bloom`] into [`ReceiptWithBloom`] instance.
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

        let this = ReceiptWithBloom {
            receipt: Self { tx_type, success, cumulative_gas_used, logs },
            logs_bloom,
        };

        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok(this)
    }

    /// Calculates the receipt root for a list of receipts.
    ///
    /// NOTE: Prefer [`crate::proofs::calculate_receipt_root`] if you have log blooms memoized.
    pub fn calculate_receipt_root_no_memo(receipts: &[Self]) -> B256 {
        ordered_trie_root_with_encoder(receipts, |r, buf| r.with_bloom_ref().encode_2718(buf))
    }
}

impl<T: TxTy> Eip2718EncodableReceipt for EthereumReceipt<T> {
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

impl<T: TxTy> Eip2718DecodableReceipt for EthereumReceipt<T> {
    fn typed_decode_with_bloom(ty: u8, buf: &mut &[u8]) -> Eip2718Result<ReceiptWithBloom<Self>> {
        Ok(Self::rlp_decode_inner(buf, T::try_from(ty)?)?)
    }

    fn fallback_decode_with_bloom(buf: &mut &[u8]) -> Eip2718Result<ReceiptWithBloom<Self>> {
        Ok(Self::rlp_decode_inner(buf, T::try_from(0)?)?)
    }
}

impl<T: TxTy> RlpEncodableReceipt for EthereumReceipt<T> {
    fn rlp_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        let payload_length = self.eip2718_encoded_length_with_bloom(bloom);

        if !self.tx_type.is_legacy() {
            payload_length + Header { list: false, payload_length }.length()
        } else {
            payload_length
        }
    }

    fn rlp_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        if !self.tx_type.is_legacy() {
            Header { list: false, payload_length: self.eip2718_encoded_length_with_bloom(bloom) }
                .encode(out);
        }
        self.eip2718_encode_with_bloom(bloom, out);
    }
}

impl<T: TxTy> RlpDecodableReceipt for EthereumReceipt<T> {
    fn rlp_decode_with_bloom(buf: &mut &[u8]) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let header_buf = &mut &**buf;
        let header = Header::decode(header_buf)?;

        // Legacy receipt, reuse initial buffer without advancing
        if header.list {
            return Self::rlp_decode_inner(buf, T::try_from(0)?);
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

impl<T: TxTy> Typed2718 for EthereumReceipt<T> {
    fn ty(&self) -> u8 {
        self.tx_type.ty()
    }
}

impl<T: TxTy + IsTyped2718> IsTyped2718 for EthereumReceipt<T> {
    fn is_type(type_id: u8) -> bool {
        <T as IsTyped2718>::is_type(type_id)
    }
}

impl<T: TxTy> InMemorySize for EthereumReceipt<T> {
    fn size(&self) -> usize {
        core::mem::size_of::<Self>() + self.logs.iter().map(|log| log.size()).sum::<usize>()
    }
}

impl<T> From<ReceiptEnvelope<T>> for EthereumReceipt<TxType>
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

impl<T, L> From<EthereumReceipt<T, L>> for crate::Receipt<L> {
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
pub(crate) mod serde_bincode_compat {
    use alloc::{borrow::Cow, vec::Vec};
    use alloy_eips::eip2718::Eip2718Error;
    use alloy_primitives::{Log, U8};
    use core::fmt::Debug;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::EthereumReceipt`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use alloy_consensus::{serde_bincode_compat, EthereumReceipt, TxType};
    /// use serde::{de::DeserializeOwned, Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::EthereumReceipt<'_>")]
    ///     receipt: EthereumReceipt<TxType>,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(bound(deserialize = "T: TryFrom<u8, Error = Eip2718Error>"))]
    pub struct EthereumReceipt<'a, T = crate::TxType> {
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

    impl<'a, T: Copy> From<&'a super::EthereumReceipt<T>> for EthereumReceipt<'a, T> {
        fn from(value: &'a super::EthereumReceipt<T>) -> Self {
            Self {
                tx_type: value.tx_type,
                success: value.success,
                cumulative_gas_used: value.cumulative_gas_used,
                logs: Cow::Borrowed(&value.logs),
            }
        }
    }

    impl<'a, T> From<EthereumReceipt<'a, T>> for super::EthereumReceipt<T> {
        fn from(value: EthereumReceipt<'a, T>) -> Self {
            Self {
                tx_type: value.tx_type,
                success: value.success,
                cumulative_gas_used: value.cumulative_gas_used,
                logs: value.logs.into_owned(),
            }
        }
    }

    impl<T: Copy + Serialize> SerializeAs<super::EthereumReceipt<T>> for EthereumReceipt<'_, T> {
        fn serialize_as<S>(
            source: &super::EthereumReceipt<T>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            EthereumReceipt::<'_>::from(source).serialize(serializer)
        }
    }

    impl<'de, T: TryFrom<u8, Error = Eip2718Error>> DeserializeAs<'de, super::EthereumReceipt<T>>
        for EthereumReceipt<'de, T>
    {
        fn deserialize_as<D>(deserializer: D) -> Result<super::EthereumReceipt<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            EthereumReceipt::<'_, T>::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use crate::TxType;
        use arbitrary::Arbitrary;
        use bincode::config;
        use rand::Rng;
        use serde_with::serde_as;

        use super::super::EthereumReceipt;

        #[test]
        fn test_ethereum_receipt_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq)]
            #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
            struct Data {
                #[serde_as(as = "super::EthereumReceipt<'_, TxType>")]
                receipt: EthereumReceipt<TxType>,
            }

            let mut bytes = [0u8; 1024];
            rand::thread_rng().fill(bytes.as_mut_slice());
            let data = Data {
                receipt: EthereumReceipt::arbitrary(&mut arbitrary::Unstructured::new(&bytes))
                    .unwrap(),
            };

            let encoded = bincode::serde::encode_to_vec(&data, config::legacy()).unwrap();
            let (decoded, _): (Data, _) =
                bincode::serde::decode_from_slice(&encoded, config::legacy()).unwrap();
            assert_eq!(decoded, data);
        }
    }
}
