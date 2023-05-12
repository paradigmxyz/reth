use crate::{bloom::logs_bloom, Bloom, Log, TxType};
use bytes::{Buf, BufMut, BytesMut};
use reth_codecs::{main_codec, Compact};
use reth_rlp::{length_of_length, Decodable, Encodable};
use std::cmp::Ordering;

/// Receipt containing result of transaction execution.
#[main_codec]
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Receipt {
    /// Receipt type.
    pub tx_type: TxType,
    /// If transaction is executed successfully.
    ///
    /// This is the `statusCode`
    pub success: bool,
    /// Gas used
    pub cumulative_gas_used: u64,
    /// Log send from contracts.
    pub logs: Vec<Log>,
}

impl Receipt {
    /// Calculates [`Log`]'s bloom filter. this is slow operation and [ReceiptWithBloom] can
    /// be used to cache this value.
    pub fn bloom_slow(&self) -> Bloom {
        logs_bloom(self.logs.iter())
    }

    /// Calculates the bloom filter for the receipt and returns the [ReceiptWithBloom] container
    /// type.
    pub fn with_bloom(self) -> ReceiptWithBloom {
        self.into()
    }
}

impl From<Receipt> for ReceiptWithBloom {
    fn from(receipt: Receipt) -> Self {
        let bloom = receipt.bloom_slow();
        ReceiptWithBloom { receipt, bloom }
    }
}

/// [`Receipt`] with calculated bloom filter.
#[main_codec]
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ReceiptWithBloom {
    /// Bloom filter build from logs.
    pub bloom: Bloom,
    /// Main receipt body
    pub receipt: Receipt,
}

impl ReceiptWithBloom {
    /// Create new [ReceiptWithBloom]
    pub fn new(receipt: Receipt, bloom: Bloom) -> Self {
        Self { receipt, bloom }
    }

    /// Consume the structure, returning only the receipt
    pub fn into_receipt(self) -> Receipt {
        self.receipt
    }

    /// Consume the structure, returning the receipt and the bloom filter
    pub fn into_components(self) -> (Receipt, Bloom) {
        (self.receipt, self.bloom)
    }

    #[inline]
    fn as_encoder(&self) -> ReceiptWithBloomEncoder<'_> {
        ReceiptWithBloomEncoder { receipt: &self.receipt, bloom: &self.bloom }
    }
}

impl ReceiptWithBloom {
    /// Encode receipt with or without the header data.
    pub fn encode_inner(&self, out: &mut dyn BufMut, with_header: bool) {
        self.as_encoder().encode_inner(out, with_header)
    }

    /// Decodes the receipt payload
    fn decode_receipt(buf: &mut &[u8], tx_type: TxType) -> Result<Self, reth_rlp::DecodeError> {
        let b = &mut &**buf;
        let rlp_head = reth_rlp::Header::decode(b)?;
        if !rlp_head.list {
            return Err(reth_rlp::DecodeError::UnexpectedString)
        }
        let started_len = b.len();

        let success = reth_rlp::Decodable::decode(b)?;
        let cumulative_gas_used = reth_rlp::Decodable::decode(b)?;
        let bloom = Decodable::decode(b)?;
        let logs = reth_rlp::Decodable::decode(b)?;

        let this = Self { receipt: Receipt { tx_type, success, cumulative_gas_used, logs }, bloom };
        let consumed = started_len - b.len();
        if consumed != rlp_head.payload_length {
            return Err(reth_rlp::DecodeError::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            })
        }
        *buf = *b;
        Ok(this)
    }
}

impl Encodable for ReceiptWithBloom {
    fn encode(&self, out: &mut dyn BufMut) {
        self.encode_inner(out, true)
    }
    fn length(&self) -> usize {
        self.as_encoder().length()
    }
}

impl Decodable for ReceiptWithBloom {
    fn decode(buf: &mut &[u8]) -> Result<Self, reth_rlp::DecodeError> {
        // a receipt is either encoded as a string (non legacy) or a list (legacy).
        // We should not consume the buffer if we are decoding a legacy receipt, so let's
        // check if the first byte is between 0x80 and 0xbf.
        let rlp_type = *buf
            .first()
            .ok_or(reth_rlp::DecodeError::Custom("cannot decode a receipt from empty bytes"))?;

        match rlp_type.cmp(&reth_rlp::EMPTY_LIST_CODE) {
            Ordering::Less => {
                // strip out the string header
                let _header = reth_rlp::Header::decode(buf)?;
                let receipt_type = *buf.first().ok_or(reth_rlp::DecodeError::Custom(
                    "typed receipt cannot be decoded from an empty slice",
                ))?;
                if receipt_type == 0x01 {
                    buf.advance(1);
                    Self::decode_receipt(buf, TxType::EIP2930)
                } else if receipt_type == 0x02 {
                    buf.advance(1);
                    Self::decode_receipt(buf, TxType::EIP1559)
                } else {
                    Err(reth_rlp::DecodeError::Custom("invalid receipt type"))
                }
            }
            Ordering::Equal => {
                Err(reth_rlp::DecodeError::Custom("an empty list is not a valid receipt encoding"))
            }
            Ordering::Greater => Self::decode_receipt(buf, TxType::Legacy),
        }
    }
}

/// [`Receipt`] reference type with calculated bloom filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptWithBloomRef<'a> {
    /// Bloom filter build from logs.
    pub bloom: Bloom,
    /// Main receipt body
    pub receipt: &'a Receipt,
}

impl<'a> ReceiptWithBloomRef<'a> {
    /// Create new [ReceiptWithBloomRef]
    pub fn new(receipt: &'a Receipt, bloom: Bloom) -> Self {
        Self { receipt, bloom }
    }

    /// Encode receipt with or without the header data.
    pub fn encode_inner(&self, out: &mut dyn BufMut, with_header: bool) {
        self.as_encoder().encode_inner(out, with_header)
    }

    #[inline]
    fn as_encoder(&self) -> ReceiptWithBloomEncoder<'_> {
        ReceiptWithBloomEncoder { receipt: self.receipt, bloom: &self.bloom }
    }
}

impl<'a> Encodable for ReceiptWithBloomRef<'a> {
    fn encode(&self, out: &mut dyn BufMut) {
        self.as_encoder().encode_inner(out, true)
    }
    fn length(&self) -> usize {
        self.as_encoder().length()
    }
}

impl<'a> From<&'a Receipt> for ReceiptWithBloomRef<'a> {
    fn from(receipt: &'a Receipt) -> Self {
        let bloom = receipt.bloom_slow();
        ReceiptWithBloomRef { receipt, bloom }
    }
}

struct ReceiptWithBloomEncoder<'a> {
    bloom: &'a Bloom,
    receipt: &'a Receipt,
}

impl<'a> ReceiptWithBloomEncoder<'a> {
    /// Returns the rlp header for the receipt payload.
    fn receipt_rlp_header(&self) -> reth_rlp::Header {
        let mut rlp_head = reth_rlp::Header { list: true, payload_length: 0 };

        rlp_head.payload_length += self.receipt.success.length();
        rlp_head.payload_length += self.receipt.cumulative_gas_used.length();
        rlp_head.payload_length += self.bloom.length();
        rlp_head.payload_length += self.receipt.logs.length();

        rlp_head
    }

    /// Encodes the receipt data.
    fn encode_fields(&self, out: &mut dyn BufMut) {
        self.receipt_rlp_header().encode(out);
        self.receipt.success.encode(out);
        self.receipt.cumulative_gas_used.encode(out);
        self.bloom.encode(out);
        self.receipt.logs.encode(out);
    }

    /// Encode receipt with or without the header data.
    fn encode_inner(&self, out: &mut dyn BufMut, with_header: bool) {
        if matches!(self.receipt.tx_type, TxType::Legacy) {
            self.encode_fields(out);
            return
        }

        let mut payload = BytesMut::new();
        self.encode_fields(&mut payload);

        if with_header {
            let payload_length = payload.len() + 1;
            let header = reth_rlp::Header { list: false, payload_length };
            header.encode(out);
        }

        match self.receipt.tx_type {
            TxType::EIP2930 => {
                out.put_u8(0x01);
            }
            TxType::EIP1559 => {
                out.put_u8(0x02);
            }
            _ => unreachable!("legacy handled; qed."),
        }
        out.put_slice(payload.as_ref());
    }

    /// Returns the length of the receipt data.
    fn receipt_length(&self) -> usize {
        let rlp_head = self.receipt_rlp_header();
        length_of_length(rlp_head.payload_length) + rlp_head.payload_length
    }
}

impl<'a> Encodable for ReceiptWithBloomEncoder<'a> {
    fn encode(&self, out: &mut dyn BufMut) {
        self.encode_inner(out, true)
    }
    fn length(&self) -> usize {
        let mut payload_len = self.receipt_length();
        // account for eip-2718 type prefix and set the list
        if matches!(self.receipt.tx_type, TxType::EIP1559 | TxType::EIP2930) {
            payload_len += 1;
            // we include a string header for typed receipts, so include the length here
            payload_len += length_of_length(payload_len);
        }

        payload_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{hex_literal::hex, Address, H256};
    use ethers_core::types::Bytes;
    use reth_rlp::{Decodable, Encodable};
    use std::str::FromStr;

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_legacy_receipt() {
        let expected = hex!("f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff");

        let mut data = vec![];
        let receipt = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 0x1u64,
                logs: vec![Log {
                    address: Address::from_str("0000000000000000000000000000000000000011").unwrap(),
                    topics: vec![
                        H256::from_str(
                            "000000000000000000000000000000000000000000000000000000000000dead",
                        )
                        .unwrap(),
                        H256::from_str(
                            "000000000000000000000000000000000000000000000000000000000000beef",
                        )
                        .unwrap(),
                    ],
                    data: Bytes::from_str("0100ff").unwrap().0.into(),
                }],
                success: false,
            },
            bloom: [0; 256].into(),
        };

        receipt.encode(&mut data);

        // check that the rlp length equals the length of the expected rlp
        assert_eq!(receipt.length(), expected.len());
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_legacy_receipt() {
        let data = hex!("f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff");

        // EIP658Receipt
        let expected = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 0x1u64,
                logs: vec![Log {
                    address: Address::from_str("0000000000000000000000000000000000000011").unwrap(),
                    topics: vec![
                        H256::from_str(
                            "000000000000000000000000000000000000000000000000000000000000dead",
                        )
                        .unwrap(),
                        H256::from_str(
                            "000000000000000000000000000000000000000000000000000000000000beef",
                        )
                        .unwrap(),
                    ],
                    data: Bytes::from_str("0100ff").unwrap().0.into(),
                }],
                success: false,
            },
            bloom: [0; 256].into(),
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
                Log {
                    address: Address::from_str("0x4bf56695415f725e43c3e04354b604bcfb6dfb6e")
                        .unwrap(),
                    topics: vec![H256::from_str(
                        "0xc69dc3d7ebff79e41f525be431d5cd3cc08f80eaf0f7819054a726eeb7086eb9",
                    )
                    .unwrap()],
                    data: crate::Bytes::from(vec![1; 0xffffff]),
                },
                Log {
                    address: Address::from_str("0xfaca325c86bf9c2d5b413cd7b90b209be92229c2")
                        .unwrap(),
                    topics: vec![H256::from_str(
                        "0x8cca58667b1e9ffa004720ac99a3d61a138181963b294d270d91c53d36402ae2",
                    )
                    .unwrap()],
                    data: crate::Bytes::from(vec![1; 0xffffff]),
                },
            ],
        };

        let mut data = vec![];
        receipt.clone().to_compact(&mut data);
        let (decoded, _) = Receipt::from_compact(&data[..], data.len());
        assert_eq!(decoded, receipt);
    }
}
