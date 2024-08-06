#[cfg(all(feature = "reth-codec", feature = "std"))]
use crate::compression::{RECEIPT_COMPRESSOR, RECEIPT_DECOMPRESSOR};
use crate::{logs_bloom, Bloom, Bytes, TxType, B256};
use alloy_primitives::Log;
use alloy_rlp::{length_of_length, Decodable, Encodable, RlpDecodable, RlpEncodable};
use bytes::{Buf, BufMut};
use core::{cmp::Ordering, ops::Deref};
use derive_more::{Deref, DerefMut, From, IntoIterator};
#[cfg(all(feature = "reth-codec", not(feature = "std")))]
use reth_codecs::Compact;
#[cfg(all(feature = "reth-codec", feature = "std"))]
use reth_codecs::{Compact, CompactZstd};
use serde::{Deserialize, Serialize};

#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};

/// Receipt containing result of transaction execution.
#[cfg_attr(
    any(test, all(feature = "reth-codec", feature = "std")),
    reth_codecs::reth_codec(no_arbitrary, zstd)
)]
#[cfg_attr(
    all(not(test), feature = "reth-codec", not(feature = "std")),
    reth_codecs::reth_codec(no_arbitrary)
)]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests)]
#[derive(
    Clone, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable, Serialize, Deserialize,
)]
#[rlp(trailing)]
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
    /// Deposit nonce for Optimism deposit transactions
    #[cfg(feature = "optimism")]
    pub deposit_nonce: Option<u64>,
    /// Deposit receipt version for Optimism deposit transactions
    ///
    ///
    /// The deposit receipt version was introduced in Canyon to indicate an update to how
    /// receipt hashes should be computed when set. The state transition process
    /// ensures this is only set for post-Canyon deposit transactions.
    #[cfg(feature = "optimism")]
    pub deposit_receipt_version: Option<u64>,
}

impl Receipt {
    /// Calculates [`Log`]'s bloom filter. this is slow operation and [`ReceiptWithBloom`] can
    /// be used to cache this value.
    pub fn bloom_slow(&self) -> Bloom {
        logs_bloom(self.logs.iter())
    }

    /// Calculates the bloom filter for the receipt and returns the [`ReceiptWithBloom`] container
    /// type.
    pub fn with_bloom(self) -> ReceiptWithBloom {
        self.into()
    }

    /// Calculates the bloom filter for the receipt and returns the [`ReceiptWithBloomRef`]
    /// container type.
    pub fn with_bloom_ref(&self) -> ReceiptWithBloomRef<'_> {
        self.into()
    }
}

/// A collection of receipts organized as a two-dimensional vector.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
    From,
    Deref,
    DerefMut,
    IntoIterator,
)]
pub struct Receipts {
    /// A two-dimensional vector of optional `Receipt` instances.
    pub receipt_vec: Vec<Vec<Option<Receipt>>>,
}

impl Receipts {
    /// Returns the length of the `Receipts` vector.
    pub fn len(&self) -> usize {
        self.receipt_vec.len()
    }

    /// Returns `true` if the `Receipts` vector is empty.
    pub fn is_empty(&self) -> bool {
        self.receipt_vec.is_empty()
    }

    /// Push a new vector of receipts into the `Receipts` collection.
    pub fn push(&mut self, receipts: Vec<Option<Receipt>>) {
        self.receipt_vec.push(receipts);
    }

    /// Retrieves the receipt root for all recorded receipts from index.
    pub fn root_slow(&self, index: usize) -> Option<B256> {
        Some(crate::proofs::calculate_receipt_root_no_memo(
            &self.receipt_vec[index].iter().map(Option::as_ref).collect::<Option<Vec<_>>>()?,
        ))
    }

    /// Retrieves the receipt root for all recorded receipts from index.
    #[cfg(feature = "optimism")]
    pub fn optimism_root_slow(
        &self,
        index: usize,
        chain_spec: &reth_chainspec::ChainSpec,
        timestamp: u64,
    ) -> Option<B256> {
        Some(crate::proofs::calculate_receipt_root_no_memo_optimism(
            &self.receipt_vec[index].iter().map(Option::as_ref).collect::<Option<Vec<_>>>()?,
            chain_spec,
            timestamp,
        ))
    }
}

impl From<Vec<Receipt>> for Receipts {
    fn from(block_receipts: Vec<Receipt>) -> Self {
        Self { receipt_vec: vec![block_receipts.into_iter().map(Option::Some).collect()] }
    }
}

impl FromIterator<Vec<Option<Receipt>>> for Receipts {
    fn from_iter<I: IntoIterator<Item = Vec<Option<Receipt>>>>(iter: I) -> Self {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}

impl From<Receipt> for ReceiptWithBloom {
    fn from(receipt: Receipt) -> Self {
        let bloom = receipt.bloom_slow();
        Self { receipt, bloom }
    }
}

/// [`Receipt`] with calculated bloom filter.
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::reth_codec)]
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReceiptWithBloom {
    /// Bloom filter build from logs.
    pub bloom: Bloom,
    /// Main receipt body
    pub receipt: Receipt,
}

impl ReceiptWithBloom {
    /// Create new [`ReceiptWithBloom`]
    pub const fn new(receipt: Receipt, bloom: Bloom) -> Self {
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
    const fn as_encoder(&self) -> ReceiptWithBloomEncoder<'_> {
        ReceiptWithBloomEncoder { receipt: &self.receipt, bloom: &self.bloom }
    }
}

/// Retrieves gas spent by transactions as a vector of tuples (transaction index, gas used).
pub fn gas_spent_by_transactions<T: Deref<Target = Receipt>>(
    receipts: impl IntoIterator<Item = T>,
) -> Vec<(u64, u64)> {
    receipts
        .into_iter()
        .enumerate()
        .map(|(id, receipt)| (id as u64, receipt.deref().cumulative_gas_used))
        .collect()
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for Receipt {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let tx_type = TxType::arbitrary(u)?;
        let success = bool::arbitrary(u)?;
        let cumulative_gas_used = u64::arbitrary(u)?;
        let logs = Vec::<Log>::arbitrary(u)?;

        // Only receipts for deposit transactions may contain a deposit nonce
        #[cfg(feature = "optimism")]
        let (deposit_nonce, deposit_receipt_version) = if tx_type == TxType::Deposit {
            let deposit_nonce = Option::<u64>::arbitrary(u)?;
            let deposit_nonce_version =
                deposit_nonce.map(|_| Option::<u64>::arbitrary(u)).transpose()?.flatten();
            (deposit_nonce, deposit_nonce_version)
        } else {
            (None, None)
        };

        Ok(Self {
            tx_type,
            success,
            cumulative_gas_used,
            logs,
            #[cfg(feature = "optimism")]
            deposit_nonce,
            #[cfg(feature = "optimism")]
            deposit_receipt_version,
        })
    }
}

impl ReceiptWithBloom {
    /// Returns the enveloped encoded receipt.
    ///
    /// See also [`ReceiptWithBloom::encode_enveloped`]
    pub fn envelope_encoded(&self) -> Bytes {
        let mut buf = Vec::new();
        self.encode_enveloped(&mut buf);
        buf.into()
    }

    /// Encodes the receipt into its "raw" format.
    /// This format is also referred to as "binary" encoding.
    ///
    /// For legacy receipts, it encodes the RLP of the receipt into the buffer:
    /// `rlp([status, cumulativeGasUsed, logsBloom, logs])` as per EIP-2718.
    /// For EIP-2718 typed transactions, it encodes the type of the transaction followed by the rlp
    /// of the receipt:
    /// - EIP-1559, 2930 and 4844 transactions: `tx-type || rlp([status, cumulativeGasUsed,
    ///   logsBloom, logs])`
    pub fn encode_enveloped(&self, out: &mut dyn bytes::BufMut) {
        self.encode_inner(out, false)
    }

    /// Encode receipt with or without the header data.
    pub fn encode_inner(&self, out: &mut dyn BufMut, with_header: bool) {
        self.as_encoder().encode_inner(out, with_header)
    }

    /// Decodes the receipt payload
    fn decode_receipt(buf: &mut &[u8], tx_type: TxType) -> alloy_rlp::Result<Self> {
        let b = &mut &**buf;
        let rlp_head = alloy_rlp::Header::decode(b)?;
        if !rlp_head.list {
            return Err(alloy_rlp::Error::UnexpectedString);
        }
        let started_len = b.len();

        let success = alloy_rlp::Decodable::decode(b)?;
        let cumulative_gas_used = alloy_rlp::Decodable::decode(b)?;
        let bloom = Decodable::decode(b)?;
        let logs = alloy_rlp::Decodable::decode(b)?;

        let receipt = match tx_type {
            #[cfg(feature = "optimism")]
            TxType::Deposit => {
                let remaining = |b: &[u8]| rlp_head.payload_length - (started_len - b.len()) > 0;
                let deposit_nonce =
                    remaining(b).then(|| alloy_rlp::Decodable::decode(b)).transpose()?;
                let deposit_receipt_version =
                    remaining(b).then(|| alloy_rlp::Decodable::decode(b)).transpose()?;

                Receipt {
                    tx_type,
                    success,
                    cumulative_gas_used,
                    logs,
                    deposit_nonce,
                    deposit_receipt_version,
                }
            }
            _ => Receipt {
                tx_type,
                success,
                cumulative_gas_used,
                logs,
                #[cfg(feature = "optimism")]
                deposit_nonce: None,
                #[cfg(feature = "optimism")]
                deposit_receipt_version: None,
            },
        };

        let this = Self { receipt, bloom };
        let consumed = started_len - b.len();
        if consumed != rlp_head.payload_length {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            });
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
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        // a receipt is either encoded as a string (non legacy) or a list (legacy).
        // We should not consume the buffer if we are decoding a legacy receipt, so let's
        // check if the first byte is between 0x80 and 0xbf.
        let rlp_type = *buf
            .first()
            .ok_or(alloy_rlp::Error::Custom("cannot decode a receipt from empty bytes"))?;

        match rlp_type.cmp(&alloy_rlp::EMPTY_LIST_CODE) {
            Ordering::Less => {
                // strip out the string header
                let _header = alloy_rlp::Header::decode(buf)?;
                let receipt_type = *buf.first().ok_or(alloy_rlp::Error::Custom(
                    "typed receipt cannot be decoded from an empty slice",
                ))?;
                match receipt_type {
                    0x01 => {
                        buf.advance(1);
                        Self::decode_receipt(buf, TxType::Eip2930)
                    }
                    0x02 => {
                        buf.advance(1);
                        Self::decode_receipt(buf, TxType::Eip1559)
                    }
                    0x03 => {
                        buf.advance(1);
                        Self::decode_receipt(buf, TxType::Eip4844)
                    }
                    0x04 => {
                        buf.advance(1);
                        Self::decode_receipt(buf, TxType::Eip7702)
                    }
                    #[cfg(feature = "optimism")]
                    0x7E => {
                        buf.advance(1);
                        Self::decode_receipt(buf, TxType::Deposit)
                    }
                    _ => Err(alloy_rlp::Error::Custom("invalid receipt type")),
                }
            }
            Ordering::Equal => {
                Err(alloy_rlp::Error::Custom("an empty list is not a valid receipt encoding"))
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
    /// Create new [`ReceiptWithBloomRef`]
    pub const fn new(receipt: &'a Receipt, bloom: Bloom) -> Self {
        Self { receipt, bloom }
    }

    /// Encode receipt with or without the header data.
    pub fn encode_inner(&self, out: &mut dyn BufMut, with_header: bool) {
        self.as_encoder().encode_inner(out, with_header)
    }

    #[inline]
    const fn as_encoder(&self) -> ReceiptWithBloomEncoder<'_> {
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
    fn receipt_rlp_header(&self) -> alloy_rlp::Header {
        let mut rlp_head = alloy_rlp::Header { list: true, payload_length: 0 };

        rlp_head.payload_length += self.receipt.success.length();
        rlp_head.payload_length += self.receipt.cumulative_gas_used.length();
        rlp_head.payload_length += self.bloom.length();
        rlp_head.payload_length += self.receipt.logs.length();

        #[cfg(feature = "optimism")]
        if self.receipt.tx_type == TxType::Deposit {
            if let Some(deposit_nonce) = self.receipt.deposit_nonce {
                rlp_head.payload_length += deposit_nonce.length();
            }
            if let Some(deposit_receipt_version) = self.receipt.deposit_receipt_version {
                rlp_head.payload_length += deposit_receipt_version.length();
            }
        }

        rlp_head
    }

    /// Encodes the receipt data.
    fn encode_fields(&self, out: &mut dyn BufMut) {
        self.receipt_rlp_header().encode(out);
        self.receipt.success.encode(out);
        self.receipt.cumulative_gas_used.encode(out);
        self.bloom.encode(out);
        self.receipt.logs.encode(out);
        #[cfg(feature = "optimism")]
        if self.receipt.tx_type == TxType::Deposit {
            if let Some(deposit_nonce) = self.receipt.deposit_nonce {
                deposit_nonce.encode(out)
            }
            if let Some(deposit_receipt_version) = self.receipt.deposit_receipt_version {
                deposit_receipt_version.encode(out)
            }
        }
    }

    /// Encode receipt with or without the header data.
    fn encode_inner(&self, out: &mut dyn BufMut, with_header: bool) {
        if matches!(self.receipt.tx_type, TxType::Legacy) {
            self.encode_fields(out);
            return;
        }

        let mut payload = Vec::new();
        self.encode_fields(&mut payload);

        if with_header {
            let payload_length = payload.len() + 1;
            let header = alloy_rlp::Header { list: false, payload_length };
            header.encode(out);
        }

        match self.receipt.tx_type {
            TxType::Legacy => unreachable!("legacy already handled"),

            TxType::Eip2930 => {
                out.put_u8(0x01);
            }
            TxType::Eip1559 => {
                out.put_u8(0x02);
            }
            TxType::Eip4844 => {
                out.put_u8(0x03);
            }
            TxType::Eip7702 => {
                out.put_u8(0x04);
            }
            #[cfg(feature = "optimism")]
            TxType::Deposit => {
                out.put_u8(0x7E);
            }
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
        if !matches!(self.receipt.tx_type, TxType::Legacy) {
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
    use crate::hex_literal::hex;
    use alloy_primitives::{address, b256, bytes};

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn encode_legacy_receipt() {
        let expected = hex!("f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff");

        let mut data = vec![];
        let receipt = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 0x1u64,
                logs: vec![Log::new_unchecked(
                    address!("0000000000000000000000000000000000000011"),
                    vec![
                        b256!("000000000000000000000000000000000000000000000000000000000000dead"),
                        b256!("000000000000000000000000000000000000000000000000000000000000beef"),
                    ],
                    bytes!("0100ff"),
                )],
                success: false,
                #[cfg(feature = "optimism")]
                deposit_nonce: None,
                #[cfg(feature = "optimism")]
                deposit_receipt_version: None,
            },
            bloom: [0; 256].into(),
        };

        receipt.encode(&mut data);

        // check that the rlp length equals the length of the expected rlp
        assert_eq!(receipt.length(), expected.len());
        assert_eq!(data, expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn decode_legacy_receipt() {
        let data = hex!("f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff");

        // EIP658Receipt
        let expected = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 0x1u64,
                logs: vec![Log::new_unchecked(
                    address!("0000000000000000000000000000000000000011"),
                    vec![
                        b256!("000000000000000000000000000000000000000000000000000000000000dead"),
                        b256!("000000000000000000000000000000000000000000000000000000000000beef"),
                    ],
                    bytes!("0100ff"),
                )],
                success: false,
                #[cfg(feature = "optimism")]
                deposit_nonce: None,
                #[cfg(feature = "optimism")]
                deposit_receipt_version: None,
            },
            bloom: [0; 256].into(),
        };

        let receipt = ReceiptWithBloom::decode(&mut &data[..]).unwrap();
        assert_eq!(receipt, expected);
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn decode_deposit_receipt_regolith_roundtrip() {
        let data = hex!("7ef9010c0182b741b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c0833d3bbf");

        // Deposit Receipt (post-regolith)
        let expected = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Deposit,
                cumulative_gas_used: 46913,
                logs: vec![],
                success: true,
                deposit_nonce: Some(4012991),
                deposit_receipt_version: None,
            },
            bloom: [0; 256].into(),
        };

        let receipt = ReceiptWithBloom::decode(&mut &data[..]).unwrap();
        assert_eq!(receipt, expected);

        let mut buf = Vec::new();
        receipt.encode_inner(&mut buf, false);
        assert_eq!(buf, &data[..]);
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn decode_deposit_receipt_canyon_roundtrip() {
        let data = hex!("7ef9010d0182b741b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c0833d3bbf01");

        // Deposit Receipt (post-regolith)
        let expected = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Deposit,
                cumulative_gas_used: 46913,
                logs: vec![],
                success: true,
                deposit_nonce: Some(4012991),
                deposit_receipt_version: Some(1),
            },
            bloom: [0; 256].into(),
        };

        let receipt = ReceiptWithBloom::decode(&mut &data[..]).unwrap();
        assert_eq!(receipt, expected);

        let mut buf = Vec::new();
        expected.encode_inner(&mut buf, false);
        assert_eq!(buf, &data[..]);
    }

    #[test]
    fn gigantic_receipt() {
        let receipt = Receipt {
            cumulative_gas_used: 16747627,
            success: true,
            tx_type: TxType::Legacy,
            logs: vec![
                Log::new_unchecked(
                    address!("4bf56695415f725e43c3e04354b604bcfb6dfb6e"),
                    vec![b256!("c69dc3d7ebff79e41f525be431d5cd3cc08f80eaf0f7819054a726eeb7086eb9")],
                    Bytes::from(vec![1; 0xffffff]),
                ),
                Log::new_unchecked(
                    address!("faca325c86bf9c2d5b413cd7b90b209be92229c2"),
                    vec![b256!("8cca58667b1e9ffa004720ac99a3d61a138181963b294d270d91c53d36402ae2")],
                    Bytes::from(vec![1; 0xffffff]),
                ),
            ],
            #[cfg(feature = "optimism")]
            deposit_nonce: None,
            #[cfg(feature = "optimism")]
            deposit_receipt_version: None,
        };

        let mut data = vec![];
        receipt.to_compact(&mut data);
        let (decoded, _) = Receipt::from_compact(&data[..], data.len());
        assert_eq!(decoded, receipt);
    }
}
