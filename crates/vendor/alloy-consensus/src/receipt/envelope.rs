use crate::{Eip658Value, Receipt, ReceiptWithBloom, TxReceipt, TxType};
use alloc::vec::Vec;
use alloy_eips::{
    eip2718::{
        Decodable2718, Eip2718Error, Eip2718Result, Encodable2718, IsTyped2718, EIP1559_TX_TYPE_ID,
        EIP2930_TX_TYPE_ID, EIP4844_TX_TYPE_ID, EIP7702_TX_TYPE_ID, LEGACY_TX_TYPE_ID,
    },
    Typed2718,
};
use alloy_primitives::{Bloom, Log};
use alloy_rlp::{BufMut, Decodable, Encodable};
use core::fmt;

/// Receipt envelope, as defined in [EIP-2718].
///
/// This enum distinguishes between tagged and untagged legacy receipts, as the
/// in-protocol Merkle tree may commit to EITHER 0-prefixed or raw. Therefore
/// we must ensure that encoding returns the precise byte-array that was
/// decoded, preserving the presence or absence of the `TransactionType` flag.
///
/// Transaction receipt payloads are specified in their respective EIPs.
///
/// [EIP-2718]: https://eips.ethereum.org/EIPS/eip-2718
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
#[cfg_attr(feature = "borsh", derive(borsh::BorshSerialize, borsh::BorshDeserialize))]
#[doc(alias = "TransactionReceiptEnvelope", alias = "TxReceiptEnvelope")]
pub enum ReceiptEnvelope<T = Log> {
    /// Receipt envelope with no type flag.
    #[cfg_attr(feature = "serde", serde(rename = "0x0", alias = "0x00"))]
    Legacy(ReceiptWithBloom<Receipt<T>>),
    /// Receipt envelope with type flag 1, containing a [EIP-2930] receipt.
    ///
    /// [EIP-2930]: https://eips.ethereum.org/EIPS/eip-2930
    #[cfg_attr(feature = "serde", serde(rename = "0x1", alias = "0x01"))]
    Eip2930(ReceiptWithBloom<Receipt<T>>),
    /// Receipt envelope with type flag 2, containing a [EIP-1559] receipt.
    ///
    /// [EIP-1559]: https://eips.ethereum.org/EIPS/eip-1559
    #[cfg_attr(feature = "serde", serde(rename = "0x2", alias = "0x02"))]
    Eip1559(ReceiptWithBloom<Receipt<T>>),
    /// Receipt envelope with type flag 3, containing a [EIP-4844] receipt.
    ///
    /// [EIP-4844]: https://eips.ethereum.org/EIPS/eip-4844
    #[cfg_attr(feature = "serde", serde(rename = "0x3", alias = "0x03"))]
    Eip4844(ReceiptWithBloom<Receipt<T>>),
    /// Receipt envelope with type flag 4, containing a [EIP-7702] receipt.
    ///
    /// [EIP-7702]: https://eips.ethereum.org/EIPS/eip-7702
    #[cfg_attr(feature = "serde", serde(rename = "0x4", alias = "0x04"))]
    Eip7702(ReceiptWithBloom<Receipt<T>>),
}

impl<T> ReceiptEnvelope<T> {
    /// Creates the envelope for a given type and receipt.
    pub fn from_typed<R>(tx_type: TxType, receipt: R) -> Self
    where
        R: Into<ReceiptWithBloom<Receipt<T>>>,
    {
        match tx_type {
            TxType::Legacy => Self::Legacy(receipt.into()),
            TxType::Eip2930 => Self::Eip2930(receipt.into()),
            TxType::Eip1559 => Self::Eip1559(receipt.into()),
            TxType::Eip4844 => Self::Eip4844(receipt.into()),
            TxType::Eip7702 => Self::Eip7702(receipt.into()),
        }
    }

    /// Converts the receipt's log type by applying a function to each log.
    ///
    /// Returns the receipt with the new log type.
    pub fn map_logs<U>(self, f: impl FnMut(T) -> U) -> ReceiptEnvelope<U> {
        match self {
            Self::Legacy(r) => ReceiptEnvelope::Legacy(r.map_logs(f)),
            Self::Eip2930(r) => ReceiptEnvelope::Eip2930(r.map_logs(f)),
            Self::Eip1559(r) => ReceiptEnvelope::Eip1559(r.map_logs(f)),
            Self::Eip4844(r) => ReceiptEnvelope::Eip4844(r.map_logs(f)),
            Self::Eip7702(r) => ReceiptEnvelope::Eip7702(r.map_logs(f)),
        }
    }

    /// Converts a [`ReceiptEnvelope`] with a custom log type into a [`ReceiptEnvelope`] with the
    /// primitives [`Log`] type by converting the logs.
    ///
    /// This is useful if log types that embed the primitives log type, e.g. the log receipt rpc
    /// type.
    pub fn into_primitives_receipt(self) -> ReceiptEnvelope<Log>
    where
        T: Into<Log>,
    {
        self.map_logs(Into::into)
    }

    /// Return the [`TxType`] of the inner receipt.
    #[doc(alias = "transaction_type")]
    pub const fn tx_type(&self) -> TxType {
        match self {
            Self::Legacy(_) => TxType::Legacy,
            Self::Eip2930(_) => TxType::Eip2930,
            Self::Eip1559(_) => TxType::Eip1559,
            Self::Eip4844(_) => TxType::Eip4844,
            Self::Eip7702(_) => TxType::Eip7702,
        }
    }

    /// Return true if the transaction was successful.
    pub const fn is_success(&self) -> bool {
        self.status()
    }

    /// Returns the success status of the receipt's transaction.
    pub const fn status(&self) -> bool {
        self.as_receipt().unwrap().status.coerce_status()
    }

    /// Returns the cumulative gas used at this receipt.
    pub const fn cumulative_gas_used(&self) -> u64 {
        self.as_receipt().unwrap().cumulative_gas_used
    }

    /// Return the receipt logs.
    pub fn logs(&self) -> &[T] {
        &self.as_receipt().unwrap().logs
    }

    /// Consumes the type and returns the logs.
    pub fn into_logs(self) -> Vec<T> {
        self.into_receipt().logs
    }

    /// Return the receipt's bloom.
    pub const fn logs_bloom(&self) -> &Bloom {
        &self.as_receipt_with_bloom().unwrap().logs_bloom
    }

    /// Return the inner receipt with bloom. Currently this is infallible,
    /// however, future receipt types may be added.
    pub const fn as_receipt_with_bloom(&self) -> Option<&ReceiptWithBloom<Receipt<T>>> {
        match self {
            Self::Legacy(t)
            | Self::Eip2930(t)
            | Self::Eip1559(t)
            | Self::Eip4844(t)
            | Self::Eip7702(t) => Some(t),
        }
    }

    /// Return the mutable inner receipt with bloom. Currently this is
    /// infallible, however, future receipt types may be added.
    pub const fn as_receipt_with_bloom_mut(&mut self) -> Option<&mut ReceiptWithBloom<Receipt<T>>> {
        match self {
            Self::Legacy(t)
            | Self::Eip2930(t)
            | Self::Eip1559(t)
            | Self::Eip4844(t)
            | Self::Eip7702(t) => Some(t),
        }
    }

    /// Consumes the type and returns the underlying [`Receipt`].
    pub fn into_receipt(self) -> Receipt<T> {
        match self {
            Self::Legacy(t)
            | Self::Eip2930(t)
            | Self::Eip1559(t)
            | Self::Eip4844(t)
            | Self::Eip7702(t) => t.receipt,
        }
    }

    /// Return the inner receipt. Currently this is infallible, however, future
    /// receipt types may be added.
    pub const fn as_receipt(&self) -> Option<&Receipt<T>> {
        match self {
            Self::Legacy(t)
            | Self::Eip2930(t)
            | Self::Eip1559(t)
            | Self::Eip4844(t)
            | Self::Eip7702(t) => Some(&t.receipt),
        }
    }
}

impl<T> TxReceipt for ReceiptEnvelope<T>
where
    T: Clone + fmt::Debug + PartialEq + Eq + Send + Sync,
{
    type Log = T;

    fn status_or_post_state(&self) -> Eip658Value {
        self.as_receipt().unwrap().status
    }

    fn status(&self) -> bool {
        self.as_receipt().unwrap().status.coerce_status()
    }

    /// Return the receipt's bloom.
    fn bloom(&self) -> Bloom {
        self.as_receipt_with_bloom().unwrap().logs_bloom
    }

    fn bloom_cheap(&self) -> Option<Bloom> {
        Some(self.bloom())
    }

    /// Returns the cumulative gas used at this receipt.
    fn cumulative_gas_used(&self) -> u64 {
        self.as_receipt().unwrap().cumulative_gas_used
    }

    /// Return the receipt logs.
    fn logs(&self) -> &[T] {
        &self.as_receipt().unwrap().logs
    }

    fn into_logs(self) -> Vec<Self::Log>
    where
        Self::Log: Clone,
    {
        self.into_receipt().logs
    }
}

impl ReceiptEnvelope {
    /// Get the length of the inner receipt in the 2718 encoding.
    pub fn inner_length(&self) -> usize {
        self.as_receipt_with_bloom().unwrap().length()
    }

    /// Calculate the length of the rlp payload of the network encoded receipt.
    pub fn rlp_payload_length(&self) -> usize {
        let length = self.as_receipt_with_bloom().unwrap().length();
        match self {
            Self::Legacy(_) => length,
            _ => length + 1,
        }
    }
}

impl Encodable for ReceiptEnvelope {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.network_encode(out)
    }

    fn length(&self) -> usize {
        self.network_len()
    }
}

impl Decodable for ReceiptEnvelope {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        Self::network_decode(buf)
            .map_or_else(|_| Err(alloy_rlp::Error::Custom("Unexpected type")), Ok)
    }
}

impl Typed2718 for ReceiptEnvelope {
    fn ty(&self) -> u8 {
        match self {
            Self::Legacy(_) => LEGACY_TX_TYPE_ID,
            Self::Eip2930(_) => EIP2930_TX_TYPE_ID,
            Self::Eip1559(_) => EIP1559_TX_TYPE_ID,
            Self::Eip4844(_) => EIP4844_TX_TYPE_ID,
            Self::Eip7702(_) => EIP7702_TX_TYPE_ID,
        }
    }
}

impl IsTyped2718 for ReceiptEnvelope {
    fn is_type(type_id: u8) -> bool {
        <TxType as IsTyped2718>::is_type(type_id)
    }
}

impl Encodable2718 for ReceiptEnvelope {
    fn encode_2718_len(&self) -> usize {
        self.inner_length() + !self.is_legacy() as usize
    }

    fn encode_2718(&self, out: &mut dyn BufMut) {
        match self.type_flag() {
            None => {}
            Some(ty) => out.put_u8(ty),
        }
        self.as_receipt_with_bloom().unwrap().encode(out);
    }
}

impl Decodable2718 for ReceiptEnvelope {
    fn typed_decode(ty: u8, buf: &mut &[u8]) -> Eip2718Result<Self> {
        let receipt = Decodable::decode(buf)?;
        match ty.try_into().map_err(|_| alloy_rlp::Error::Custom("Unexpected type"))? {
            TxType::Eip2930 => Ok(Self::Eip2930(receipt)),
            TxType::Eip1559 => Ok(Self::Eip1559(receipt)),
            TxType::Eip4844 => Ok(Self::Eip4844(receipt)),
            TxType::Eip7702 => Ok(Self::Eip7702(receipt)),
            TxType::Legacy => Err(Eip2718Error::UnexpectedType(0)),
        }
    }

    fn fallback_decode(buf: &mut &[u8]) -> Eip2718Result<Self> {
        Ok(Self::Legacy(Decodable::decode(buf)?))
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a, T> arbitrary::Arbitrary<'a> for ReceiptEnvelope<T>
where
    T: arbitrary::Arbitrary<'a>,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let receipt = ReceiptWithBloom::<Receipt<T>>::arbitrary(u)?;

        match u.int_in_range(0..=4)? {
            0 => Ok(Self::Legacy(receipt)),
            1 => Ok(Self::Eip2930(receipt)),
            2 => Ok(Self::Eip1559(receipt)),
            3 => Ok(Self::Eip4844(receipt)),
            4 => Ok(Self::Eip7702(receipt)),
            _ => unreachable!(),
        }
    }
}

/// Bincode-compatible [`ReceiptEnvelope`] serde implementation.
#[cfg(all(feature = "serde", feature = "serde-bincode-compat"))]
pub(crate) mod serde_bincode_compat {
    use crate::{Receipt, ReceiptWithBloom, TxType};
    use alloc::borrow::Cow;
    use alloy_primitives::{Bloom, Log, U8};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::ReceiptEnvelope`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use alloy_consensus::{serde_bincode_compat, ReceiptEnvelope};
    /// use serde::{de::DeserializeOwned, Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data<T: Serialize + DeserializeOwned + Clone + 'static> {
    ///     #[serde_as(as = "serde_bincode_compat::ReceiptEnvelope<'_, T>")]
    ///     receipt: ReceiptEnvelope<T>,
    /// }
    /// ```
    #[derive(Debug, Serialize, Deserialize)]
    pub struct ReceiptEnvelope<'a, T: Clone = Log> {
        #[serde(deserialize_with = "deserde_txtype")]
        tx_type: TxType,
        success: bool,
        cumulative_gas_used: u64,
        logs_bloom: Cow<'a, Bloom>,
        logs: Cow<'a, [T]>,
    }

    /// Ensures that txtype is deserialized symmetrically as U8
    fn deserde_txtype<'de, D>(deserializer: D) -> Result<TxType, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = U8::deserialize(deserializer)?;
        value.to::<u8>().try_into().map_err(serde::de::Error::custom)
    }

    impl<'a, T: Clone> From<&'a super::ReceiptEnvelope<T>> for ReceiptEnvelope<'a, T> {
        fn from(value: &'a super::ReceiptEnvelope<T>) -> Self {
            Self {
                tx_type: value.tx_type(),
                success: value.status(),
                cumulative_gas_used: value.cumulative_gas_used(),
                logs_bloom: Cow::Borrowed(value.logs_bloom()),
                logs: Cow::Borrowed(value.logs()),
            }
        }
    }

    impl<'a, T: Clone> From<ReceiptEnvelope<'a, T>> for super::ReceiptEnvelope<T> {
        fn from(value: ReceiptEnvelope<'a, T>) -> Self {
            let ReceiptEnvelope { tx_type, success, cumulative_gas_used, logs_bloom, logs } = value;
            let receipt = ReceiptWithBloom {
                receipt: Receipt {
                    status: success.into(),
                    cumulative_gas_used,
                    logs: logs.into_owned(),
                },
                logs_bloom: logs_bloom.into_owned(),
            };
            match tx_type {
                TxType::Legacy => Self::Legacy(receipt),
                TxType::Eip2930 => Self::Eip2930(receipt),
                TxType::Eip1559 => Self::Eip1559(receipt),
                TxType::Eip4844 => Self::Eip4844(receipt),
                TxType::Eip7702 => Self::Eip7702(receipt),
            }
        }
    }

    impl<T: Serialize + Clone> SerializeAs<super::ReceiptEnvelope<T>> for ReceiptEnvelope<'_, T> {
        fn serialize_as<S>(
            source: &super::ReceiptEnvelope<T>,
            serializer: S,
        ) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            ReceiptEnvelope::<'_, T>::from(source).serialize(serializer)
        }
    }

    impl<'de, T: Deserialize<'de> + Clone> DeserializeAs<'de, super::ReceiptEnvelope<T>>
        for ReceiptEnvelope<'de, T>
    {
        fn deserialize_as<D>(deserializer: D) -> Result<super::ReceiptEnvelope<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            ReceiptEnvelope::<'_, T>::deserialize(deserializer).map(Into::into)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::{serde_bincode_compat, ReceiptEnvelope};
        use alloy_primitives::Log;
        use arbitrary::Arbitrary;
        use bincode::config;
        use rand::Rng;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_receipt_envelope_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::ReceiptEnvelope<'_>")]
                transaction: ReceiptEnvelope<Log>,
            }

            let mut bytes = [0u8; 1024];
            rand::thread_rng().fill(bytes.as_mut_slice());
            let mut data = Data {
                transaction: ReceiptEnvelope::arbitrary(&mut arbitrary::Unstructured::new(&bytes))
                    .unwrap(),
            };

            // ensure we have proper roundtrip data
            data.transaction.as_receipt_with_bloom_mut().unwrap().receipt.status = true.into();

            let encoded = bincode::serde::encode_to_vec(&data, config::legacy()).unwrap();
            let (decoded, _) =
                bincode::serde::decode_from_slice::<Data, _>(&encoded, config::legacy()).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{Receipt, ReceiptEnvelope, TxType};
    use alloy_primitives::Log;

    #[cfg(feature = "serde")]
    #[test]
    fn deser_pre658_receipt_envelope() {
        use crate::Receipt;
        use alloy_primitives::b256;

        let receipt = super::ReceiptWithBloom::<Receipt<()>> {
            receipt: super::Receipt {
                status: super::Eip658Value::PostState(b256!(
                    "284d35bf53b82ef480ab4208527325477439c64fb90ef518450f05ee151c8e10"
                )),
                cumulative_gas_used: 0,
                logs: Default::default(),
            },
            logs_bloom: Default::default(),
        };

        let json = serde_json::to_string(&receipt).unwrap();

        println!("Serialized {json}");

        let receipt: super::ReceiptWithBloom<Receipt<()>> = serde_json::from_str(&json).unwrap();

        assert_eq!(
            receipt.receipt.status,
            super::Eip658Value::PostState(b256!(
                "284d35bf53b82ef480ab4208527325477439c64fb90ef518450f05ee151c8e10"
            ))
        );
    }

    #[test]
    fn convert_envelope() {
        let receipt = Receipt::<Log>::default();
        let _envelope = ReceiptEnvelope::from_typed(TxType::Eip7702, receipt);
    }
}
