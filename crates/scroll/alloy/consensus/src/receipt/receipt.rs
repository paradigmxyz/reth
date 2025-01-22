//! Transaction receipt types for Scroll.

use alloy_consensus::{
    Eip658Value, Receipt, ReceiptWithBloom, RlpDecodableReceipt, RlpEncodableReceipt, TxReceipt,
};
use alloy_primitives::{Bloom, Log, U256};
use alloy_rlp::{Buf, BufMut, Decodable, Encodable, Header};

/// Receipt containing result of transaction execution.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct ScrollTransactionReceipt<T = Log> {
    /// The inner receipt type.
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub inner: Receipt<T>,
    /// L1 fee for Scroll transactions.
    pub l1_fee: U256,
}

impl<T> ScrollTransactionReceipt<T> {
    /// Returns a new [`ScrollTransactionReceipt`] from the inner receipt and the l1 fee.
    pub const fn new(inner: Receipt<T>, l1_fee: U256) -> Self {
        Self { inner, l1_fee }
    }
}

impl ScrollTransactionReceipt {
    /// Calculates [`Log`]'s bloom filter. This is slow operation and [`ScrollReceiptWithBloom`]
    /// can be used to cache this value.
    pub fn bloom_slow(&self) -> Bloom {
        self.inner.logs.iter().collect()
    }

    /// Calculates the bloom filter for the receipt and returns the [`ScrollReceiptWithBloom`]
    /// container type.
    pub fn with_bloom(self) -> ScrollReceiptWithBloom {
        self.into()
    }
}

impl<T: Encodable> ScrollTransactionReceipt<T> {
    /// Returns length of RLP-encoded receipt fields with the given [`Bloom`] without an RLP header.
    /// Does not include the L1 fee field which is not part of the consensus encoding of a receipt.
    /// <https://github.com/scroll-tech/go-ethereum/blob/9fff27e4f34fb5097100ed76ee725ce056267f4b/core/types/receipt.go#L96-L102>
    pub fn rlp_encoded_fields_length_with_bloom(&self, bloom: &Bloom) -> usize {
        self.inner.rlp_encoded_fields_length_with_bloom(bloom)
    }

    /// RLP-encodes receipt fields with the given [`Bloom`] without an RLP header.
    /// Does not include the L1 fee field which is not part of the consensus encoding of a receipt.
    /// <https://github.com/scroll-tech/go-ethereum/blob/9fff27e4f34fb5097100ed76ee725ce056267f4b/core/types/receipt.go#L96-L102>
    pub fn rlp_encode_fields_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        self.inner.rlp_encode_fields_with_bloom(bloom, out);
    }

    /// Returns RLP header for this receipt encoding with the given [`Bloom`].
    /// Does not include the L1 fee field which is not part of the consensus encoding of a receipt.
    /// <https://github.com/scroll-tech/go-ethereum/blob/9fff27e4f34fb5097100ed76ee725ce056267f4b/core/types/receipt.go#L96-L102>
    pub fn rlp_header_with_bloom(&self, bloom: &Bloom) -> Header {
        Header { list: true, payload_length: self.rlp_encoded_fields_length_with_bloom(bloom) }
    }
}

impl<T: Decodable> ScrollTransactionReceipt<T> {
    /// RLP-decodes receipt's field with a [`Bloom`].
    ///
    /// Does not expect an RLP header.
    pub fn rlp_decode_fields_with_bloom(
        buf: &mut &[u8],
    ) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let ReceiptWithBloom { receipt: inner, logs_bloom } =
            Receipt::rlp_decode_fields_with_bloom(buf)?;

        Ok(ReceiptWithBloom { logs_bloom, receipt: Self { inner, l1_fee: Default::default() } })
    }
}

impl<T> AsRef<Receipt<T>> for ScrollTransactionReceipt<T> {
    fn as_ref(&self) -> &Receipt<T> {
        &self.inner
    }
}

impl<T> TxReceipt for ScrollTransactionReceipt<T>
where
    T: AsRef<Log> + Clone + core::fmt::Debug + PartialEq + Eq + Send + Sync,
{
    type Log = T;

    fn status_or_post_state(&self) -> Eip658Value {
        self.inner.status_or_post_state()
    }

    fn status(&self) -> bool {
        self.inner.status()
    }

    fn bloom(&self) -> Bloom {
        self.inner.bloom_slow()
    }

    fn cumulative_gas_used(&self) -> u64 {
        self.inner.cumulative_gas_used()
    }

    fn logs(&self) -> &[Self::Log] {
        self.inner.logs()
    }
}

impl<T: Encodable> RlpEncodableReceipt for ScrollTransactionReceipt<T> {
    fn rlp_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        self.rlp_header_with_bloom(bloom).length_with_payload()
    }

    fn rlp_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        self.rlp_header_with_bloom(bloom).encode(out);
        self.rlp_encode_fields_with_bloom(bloom, out);
    }
}

impl<T: Decodable> RlpDecodableReceipt for ScrollTransactionReceipt<T> {
    fn rlp_decode_with_bloom(buf: &mut &[u8]) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let header = Header::decode(buf)?;
        if !header.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }

        if buf.len() < header.payload_length {
            return Err(alloy_rlp::Error::InputTooShort);
        }

        // Note: we pass a separate buffer to `rlp_decode_fields_with_bloom` to allow it decode
        // optional fields based on the remaining length.
        let mut fields_buf = &buf[..header.payload_length];
        let this = Self::rlp_decode_fields_with_bloom(&mut fields_buf)?;

        if !fields_buf.is_empty() {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        buf.advance(header.payload_length);

        Ok(this)
    }
}

/// [`ScrollTransactionReceipt`] with calculated bloom filter, modified for Scroll.
///
/// This convenience type allows us to lazily calculate the bloom filter for a
/// receipt, similar to [`Sealed`].
///
/// [`Sealed`]: alloy_consensus::Sealed
pub type ScrollReceiptWithBloom<T = Log> = ReceiptWithBloom<ScrollTransactionReceipt<T>>;

#[cfg(any(test, feature = "arbitrary"))]
impl<'a, T> arbitrary::Arbitrary<'a> for ScrollTransactionReceipt<T>
where
    T: arbitrary::Arbitrary<'a>,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        #[cfg(not(feature = "std"))]
        use alloc::vec::Vec;
        Ok(Self {
            inner: Receipt {
                status: Eip658Value::arbitrary(u)?,
                cumulative_gas_used: u64::arbitrary(u)?,
                logs: Vec::<T>::arbitrary(u)?,
            },
            l1_fee: U256::arbitrary(u)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Receipt;
    use alloy_primitives::{address, b256, bytes, hex, Bytes, Log, LogData};
    use alloy_rlp::{Decodable, Encodable};

    #[cfg(not(feature = "std"))]
    use alloc::{vec, vec::Vec};

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn decode_legacy_receipt() {
        let data = hex!("f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff");

        // EIP658Receipt
        let expected =
            ScrollReceiptWithBloom {
                receipt: ScrollTransactionReceipt {
                    inner: Receipt {
                        status: false.into(),
                        cumulative_gas_used: 0x1,
                        logs: vec![Log {
                            address: address!("0000000000000000000000000000000000000011"),
                            data: LogData::new_unchecked(
                                vec![
                                    b256!("000000000000000000000000000000000000000000000000000000000000dead"),
                                    b256!("000000000000000000000000000000000000000000000000000000000000beef"),
                                ],
                                bytes!("0100ff"),
                            ),
                        }],
                    },
                    l1_fee: U256::ZERO
                },
                logs_bloom: [0; 256].into(),
            };

        let receipt = ScrollReceiptWithBloom::decode(&mut &data[..]).unwrap();
        assert_eq!(receipt, expected);
    }

    #[test]
    fn gigantic_receipt() {
        let receipt = ScrollTransactionReceipt {
            inner: Receipt {
                cumulative_gas_used: 16747627,
                status: true.into(),
                logs: vec![
                    Log {
                        address: address!("4bf56695415f725e43c3e04354b604bcfb6dfb6e"),
                        data: LogData::new_unchecked(
                            vec![b256!(
                                "c69dc3d7ebff79e41f525be431d5cd3cc08f80eaf0f7819054a726eeb7086eb9"
                            )],
                            Bytes::from(vec![1; 0xffffff]),
                        ),
                    },
                    Log {
                        address: address!("faca325c86bf9c2d5b413cd7b90b209be92229c2"),
                        data: LogData::new_unchecked(
                            vec![b256!(
                                "8cca58667b1e9ffa004720ac99a3d61a138181963b294d270d91c53d36402ae2"
                            )],
                            Bytes::from(vec![1; 0xffffff]),
                        ),
                    },
                ],
            },
            l1_fee: U256::ZERO,
        }
        .with_bloom();

        let mut data = vec![];

        receipt.encode(&mut data);
        let decoded = ScrollReceiptWithBloom::decode(&mut &data[..]).unwrap();

        assert_eq!(decoded, receipt);
    }
}
