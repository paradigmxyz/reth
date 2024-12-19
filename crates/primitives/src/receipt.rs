use alloc::{vec, vec::Vec};
use reth_primitives_traits::InMemorySize;

use alloy_consensus::{
    Eip2718EncodableReceipt, Eip658Value, ReceiptWithBloom, RlpDecodableReceipt,
    RlpEncodableReceipt, TxReceipt, Typed2718,
};
use alloy_primitives::{Bloom, Log, B256};
use alloy_rlp::{Decodable, Encodable, Header, RlpDecodable, RlpEncodable};
use bytes::BufMut;
use derive_more::{DerefMut, From, IntoIterator};
use serde::{Deserialize, Serialize};

use crate::TxType;

/// Retrieves gas spent by transactions as a vector of tuples (transaction index, gas used).
pub use reth_primitives_traits::receipt::gas_spent_by_transactions;

/// Receipt containing result of transaction execution.
#[derive(
    Clone, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable, Serialize, Deserialize,
)]
#[cfg_attr(any(test, feature = "reth-codec"), derive(reth_codecs::CompactZstd))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_zstd(
    compressor = reth_zstd_compressors::RECEIPT_COMPRESSOR,
    decompressor = reth_zstd_compressors::RECEIPT_DECOMPRESSOR
))]
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
        alloy_primitives::logs_bloom(self.logs.iter())
    }

    /// Calculates the bloom filter for the receipt and returns the [`ReceiptWithBloom`] container
    /// type.
    pub fn with_bloom(self) -> ReceiptWithBloom<Self> {
        self.into()
    }

    /// Calculates the bloom filter for the receipt and returns the [`ReceiptWithBloom`]
    /// container type.
    pub fn with_bloom_ref(&self) -> ReceiptWithBloom<&Self> {
        self.into()
    }

    /// Returns length of RLP-encoded receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encoded_fields_length(&self, bloom: &Bloom) -> usize {
        let len = self.success.length() +
            self.cumulative_gas_used.length() +
            bloom.length() +
            self.logs.length();

        #[cfg(feature = "optimism")]
        if self.tx_type == TxType::Deposit {
            let mut len = len;

            if let Some(deposit_nonce) = self.deposit_nonce {
                len += deposit_nonce.length();
            }
            if let Some(deposit_receipt_version) = self.deposit_receipt_version {
                len += deposit_receipt_version.length();
            }

            return len
        }

        len
    }

    /// RLP-encodes receipt fields with the given [`Bloom`] without an RLP header.
    pub fn rlp_encode_fields(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        self.success.encode(out);
        self.cumulative_gas_used.encode(out);
        bloom.encode(out);
        self.logs.encode(out);

        #[cfg(feature = "optimism")]
        if self.tx_type == TxType::Deposit {
            if let Some(nonce) = self.deposit_nonce {
                nonce.encode(out);
            }
            if let Some(version) = self.deposit_receipt_version {
                version.encode(out);
            }
        }
    }

    /// Returns RLP header for inner encoding.
    pub fn rlp_header_inner(&self, bloom: &Bloom) -> Header {
        Header { list: true, payload_length: self.rlp_encoded_fields_length(bloom) }
    }

    fn decode_receipt_with_bloom(
        buf: &mut &[u8],
        tx_type: TxType,
    ) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let b = &mut &**buf;
        let rlp_head = alloy_rlp::Header::decode(b)?;
        if !rlp_head.list {
            return Err(alloy_rlp::Error::UnexpectedString)
        }
        let started_len = b.len();

        let success = Decodable::decode(b)?;
        let cumulative_gas_used = Decodable::decode(b)?;
        let bloom = Decodable::decode(b)?;
        let logs = Decodable::decode(b)?;

        let receipt = match tx_type {
            #[cfg(feature = "optimism")]
            TxType::Deposit => {
                let remaining = |b: &[u8]| rlp_head.payload_length - (started_len - b.len()) > 0;
                let deposit_nonce = remaining(b).then(|| Decodable::decode(b)).transpose()?;
                let deposit_receipt_version =
                    remaining(b).then(|| Decodable::decode(b)).transpose()?;

                Self {
                    tx_type,
                    success,
                    cumulative_gas_used,
                    logs,
                    deposit_nonce,
                    deposit_receipt_version,
                }
            }
            _ => Self {
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

        let this = ReceiptWithBloom { receipt, logs_bloom: bloom };
        let consumed = started_len - b.len();
        if consumed != rlp_head.payload_length {
            return Err(alloy_rlp::Error::ListLengthMismatch {
                expected: rlp_head.payload_length,
                got: consumed,
            })
        }
        *buf = *b;
        Ok(this)
    }
}

impl Eip2718EncodableReceipt for Receipt {
    fn eip2718_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        self.rlp_header_inner(bloom).length_with_payload() +
            !matches!(self.tx_type, TxType::Legacy) as usize // account for type prefix
    }

    fn eip2718_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        if !matches!(self.tx_type, TxType::Legacy) {
            out.put_u8(self.tx_type as u8);
        }
        self.rlp_header_inner(bloom).encode(out);
        self.rlp_encode_fields(bloom, out);
    }
}

impl RlpEncodableReceipt for Receipt {
    fn rlp_encoded_length_with_bloom(&self, bloom: &Bloom) -> usize {
        let mut len = self.eip2718_encoded_length_with_bloom(bloom);
        if !matches!(self.tx_type, TxType::Legacy) {
            len += Header {
                list: false,
                payload_length: self.eip2718_encoded_length_with_bloom(bloom),
            }
            .length();
        }

        len
    }

    fn rlp_encode_with_bloom(&self, bloom: &Bloom, out: &mut dyn BufMut) {
        if !matches!(self.tx_type, TxType::Legacy) {
            Header { list: false, payload_length: self.eip2718_encoded_length_with_bloom(bloom) }
                .encode(out);
        }
        self.eip2718_encode_with_bloom(bloom, out);
    }
}

impl RlpDecodableReceipt for Receipt {
    fn rlp_decode_with_bloom(buf: &mut &[u8]) -> alloy_rlp::Result<ReceiptWithBloom<Self>> {
        let header_buf = &mut &**buf;
        let header = Header::decode(header_buf)?;

        if header.list {
            return Self::decode_receipt_with_bloom(buf, TxType::Legacy);
        }

        *buf = *header_buf;

        let remaining = buf.len();
        let tx_type = TxType::decode(buf)?;
        let this = Self::decode_receipt_with_bloom(buf, tx_type)?;

        if buf.len() + header.payload_length != remaining {
            return Err(alloy_rlp::Error::UnexpectedLength);
        }

        Ok(this)
    }
}

impl TxReceipt for Receipt {
    type Log = Log;

    fn status_or_post_state(&self) -> Eip658Value {
        self.success.into()
    }

    fn status(&self) -> bool {
        self.success
    }

    fn bloom(&self) -> Bloom {
        alloy_primitives::logs_bloom(self.logs.iter())
    }

    fn cumulative_gas_used(&self) -> u128 {
        self.cumulative_gas_used as u128
    }

    fn logs(&self) -> &[Log] {
        &self.logs
    }
}

impl Typed2718 for Receipt {
    fn ty(&self) -> u8 {
        self.tx_type as u8
    }
}

impl reth_primitives_traits::Receipt for Receipt {}

impl InMemorySize for Receipt {
    /// Calculates a heuristic for the in-memory size of the [Receipt].
    #[inline]
    fn size(&self) -> usize {
        let total_size = self.tx_type.size() +
            core::mem::size_of::<bool>() +
            core::mem::size_of::<u64>() +
            self.logs.capacity() * core::mem::size_of::<Log>();

        #[cfg(feature = "optimism")]
        return total_size + 2 * core::mem::size_of::<Option<u64>>();
        #[cfg(not(feature = "optimism"))]
        total_size
    }
}

/// A collection of receipts organized as a two-dimensional vector.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    From,
    derive_more::Deref,
    DerefMut,
    IntoIterator,
)]
pub struct Receipts<T = Receipt> {
    /// A two-dimensional vector of optional `Receipt` instances.
    pub receipt_vec: Vec<Vec<Option<T>>>,
}

impl<T> Receipts<T> {
    /// Returns the length of the `Receipts` vector.
    pub fn len(&self) -> usize {
        self.receipt_vec.len()
    }

    /// Returns true if the `Receipts` vector is empty.
    pub fn is_empty(&self) -> bool {
        self.receipt_vec.is_empty()
    }

    /// Push a new vector of receipts into the `Receipts` collection.
    pub fn push(&mut self, receipts: Vec<Option<T>>) {
        self.receipt_vec.push(receipts);
    }

    /// Retrieves all recorded receipts from index and calculates the root using the given closure.
    pub fn root_slow(&self, index: usize, f: impl FnOnce(&[&T]) -> B256) -> Option<B256> {
        let receipts =
            self.receipt_vec[index].iter().map(Option::as_ref).collect::<Option<Vec<_>>>()?;
        Some(f(receipts.as_slice()))
    }
}

impl<T> From<Vec<T>> for Receipts<T> {
    fn from(block_receipts: Vec<T>) -> Self {
        Self { receipt_vec: vec![block_receipts.into_iter().map(Option::Some).collect()] }
    }
}

impl<T> FromIterator<Vec<Option<T>>> for Receipts<T> {
    fn from_iter<I: IntoIterator<Item = Vec<Option<T>>>>(iter: I) -> Self {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}

impl<T> Default for Receipts<T> {
    fn default() -> Self {
        Self { receipt_vec: Vec::new() }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eips::eip2718::Encodable2718;
    use alloy_primitives::{address, b256, bytes, hex_literal::hex, Bytes};
    use reth_codecs::Compact;

    #[test]
    fn test_decode_receipt() {
        #[cfg(not(feature = "optimism"))]
        reth_codecs::test_utils::test_decode::<Receipt>(&hex!(
            "c428b52ffd23fc42696156b10200f034792b6a94c3850215c2fef7aea361a0c31b79d9a32652eefc0d4e2e730036061cff7344b6fc6132b50cda0ed810a991ae58ef013150c12b2522533cb3b3a8b19b7786a8b5ff1d3cdc84225e22b02def168c8858df"
        ));
        #[cfg(feature = "optimism")]
        reth_codecs::test_utils::test_decode::<Receipt>(&hex!(
            "c30328b52ffd23fc426961a00105007eb0042307705a97e503562eacf2b95060cce9de6de68386b6c155b73a9650021a49e2f8baad17f30faff5899d785c4c0873e45bc268bcf07560106424570d11f9a59e8f3db1efa4ceec680123712275f10d92c3411e1caaa11c7c5d591bc11487168e09934a9986848136da1b583babf3a7188e3aed007a1520f1cf4c1ca7d3482c6c28d37c298613c70a76940008816c4c95644579fd08471dc34732fd0f24"
        ));
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn encode_legacy_receipt() {
        let expected = hex!("f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff");

        let mut data = Vec::with_capacity(expected.length());
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
            logs_bloom: [0; 256].into(),
        };

        let receipt = ReceiptWithBloom::decode(&mut &data[..]).unwrap();
        assert_eq!(receipt, expected);
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn decode_deposit_receipt_regolith_roundtrip() {
        let data = hex!("b901107ef9010c0182b741b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c0833d3bbf");

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
            logs_bloom: [0; 256].into(),
        };

        let receipt = ReceiptWithBloom::decode(&mut &data[..]).unwrap();
        assert_eq!(receipt, expected);

        let mut buf = Vec::with_capacity(data.len());
        receipt.encode(&mut buf);
        assert_eq!(buf, &data[..]);
    }

    #[cfg(feature = "optimism")]
    #[test]
    fn decode_deposit_receipt_canyon_roundtrip() {
        let data = hex!("b901117ef9010d0182b741b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000c0833d3bbf01");

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
            logs_bloom: [0; 256].into(),
        };

        let receipt = ReceiptWithBloom::decode(&mut &data[..]).unwrap();
        assert_eq!(receipt, expected);

        let mut buf = Vec::with_capacity(data.len());
        expected.encode(&mut buf);
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

    #[test]
    fn test_encode_2718_length() {
        let receipt = ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::Eip1559,
                success: true,
                cumulative_gas_used: 21000,
                logs: vec![],
                #[cfg(feature = "optimism")]
                deposit_nonce: None,
                #[cfg(feature = "optimism")]
                deposit_receipt_version: None,
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
                #[cfg(feature = "optimism")]
                deposit_nonce: None,
                #[cfg(feature = "optimism")]
                deposit_receipt_version: None,
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
}
