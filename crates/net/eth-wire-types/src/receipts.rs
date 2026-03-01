//! Implements the `GetReceipts` and `Receipts` message types.

use alloc::vec::Vec;
use alloy_consensus::{ReceiptWithBloom, RlpDecodableReceipt, RlpEncodableReceipt, TxReceipt};
use alloy_primitives::B256;
use alloy_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
use derive_more::{Deref, IntoIterator};
use reth_codecs_derive::add_arbitrary_tests;
use reth_ethereum_primitives::Receipt;

/// A request for transaction receipts from the given block hashes.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    Default,
    Deref,
    IntoIterator,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetReceipts(
    /// The block hashes to request receipts for.
    pub Vec<B256>,
);

/// Eth/70 `GetReceipts` request payload that supports partial receipt queries.
///
/// When used with eth/70, the request id is carried by the surrounding
/// [`crate::message::RequestPair`], and the on-wire shape is the flattened list
/// `firstBlockReceiptIndex, [blockhash₁, ...]`.
///
/// See also [eip-7975](https://eips.ethereum.org/EIPS/eip-7975)
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub struct GetReceipts70 {
    /// Index into the receipts of the first requested block hash.
    pub first_block_receipt_index: u64,
    /// The block hashes to request receipts for.
    pub block_hashes: Vec<B256>,
}

impl alloy_rlp::Encodable for GetReceipts70 {
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.first_block_receipt_index.encode(out);
        self.block_hashes.encode(out);
    }

    fn length(&self) -> usize {
        self.first_block_receipt_index.length() + self.block_hashes.length()
    }
}

impl alloy_rlp::Decodable for GetReceipts70 {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let first_block_receipt_index = u64::decode(buf)?;
        let block_hashes = Vec::<B256>::decode(buf)?;
        Ok(Self { first_block_receipt_index, block_hashes })
    }
}

/// The response to [`GetReceipts`], containing receipt lists that correspond to each block
/// requested.
#[derive(Clone, Debug, PartialEq, Eq, Default, Deref, IntoIterator)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct Receipts<T = Receipt>(
    /// Each receipt hash should correspond to a block hash in the request.
    pub Vec<Vec<ReceiptWithBloom<T>>>,
);

impl<T: RlpEncodableReceipt> alloy_rlp::Encodable for Receipts<T> {
    #[inline]
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.0.encode(out)
    }
    #[inline]
    fn length(&self) -> usize {
        self.0.length()
    }
}

impl<T: RlpDecodableReceipt> alloy_rlp::Decodable for Receipts<T> {
    #[inline]
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        alloy_rlp::Decodable::decode(buf).map(Self)
    }
}

/// Eth/69 receipt response type that removes bloom filters from the protocol.
///
/// This is effectively a subset of [`Receipts`].
#[derive(
    Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Deref, IntoIterator,
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct Receipts69<T = Receipt>(pub Vec<Vec<T>>);

impl<T: TxReceipt> Receipts69<T> {
    /// Encodes all receipts with the bloom filter.
    ///
    /// Eth/69 omits bloom filters on the wire, while some internal callers
    /// (and legacy APIs) still operate on [`Receipts`] with
    /// [`ReceiptWithBloom`]. This helper reconstructs the bloom locally from
    /// each receipt's logs so the older API can be used on top of eth/69 data.
    ///
    /// Note: This is an expensive operation that recalculates the bloom for
    /// every receipt.
    pub fn into_with_bloom(self) -> Receipts<T> {
        Receipts(
            self.into_iter()
                .map(|receipts| receipts.into_iter().map(|r| r.into_with_bloom()).collect())
                .collect(),
        )
    }
}

impl<T: TxReceipt> From<Receipts69<T>> for Receipts<T> {
    fn from(receipts: Receipts69<T>) -> Self {
        receipts.into_with_bloom()
    }
}

/// Eth/70 `Receipts` response payload.
///
/// This is used in conjunction with [`crate::message::RequestPair`] to encode the full wire
/// message `[request-id, lastBlockIncomplete, [[receipt₁, receipt₂], ...]]`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub struct Receipts70<T = Receipt> {
    /// Whether the receipts list for the last block is incomplete.
    pub last_block_incomplete: bool,
    /// Receipts grouped by block.
    pub receipts: Vec<Vec<T>>,
}

impl<T> alloy_rlp::Encodable for Receipts70<T>
where
    T: alloy_rlp::Encodable,
{
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        self.last_block_incomplete.encode(out);
        self.receipts.encode(out);
    }

    fn length(&self) -> usize {
        self.last_block_incomplete.length() + self.receipts.length()
    }
}

impl<T> alloy_rlp::Decodable for Receipts70<T>
where
    T: alloy_rlp::Decodable,
{
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let last_block_incomplete = bool::decode(buf)?;
        let receipts = Vec::<Vec<T>>::decode(buf)?;
        Ok(Self { last_block_incomplete, receipts })
    }
}

impl<T: TxReceipt> Receipts70<T> {
    /// Encodes all receipts with the bloom filter.
    ///
    /// Just like eth/69, eth/70 does not transmit bloom filters over the wire.
    /// When higher layers still expect the older bloom-bearing [`Receipts`]
    /// type, this helper converts the eth/70 payload into that shape by
    /// recomputing the bloom locally from the contained receipts.
    ///
    /// Note: This is an expensive operation that recalculates the bloom for
    /// every receipt.
    pub fn into_with_bloom(self) -> Receipts<T> {
        // Reuse the eth/69 helper, since both variants carry the same
        // receipt list shape (only eth/70 adds request metadata).
        Receipts69(self.receipts).into_with_bloom()
    }
}

impl<T: TxReceipt> From<Receipts70<T>> for Receipts<T> {
    fn from(receipts: Receipts70<T>) -> Self {
        receipts.into_with_bloom()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{message::RequestPair, GetReceipts, Receipts};
    use alloy_consensus::TxType;
    use alloy_primitives::{hex, Log};
    use alloy_rlp::{Decodable, Encodable};

    #[test]
    fn roundtrip_eip1559() {
        let receipts = Receipts(vec![vec![ReceiptWithBloom {
            receipt: Receipt { tx_type: TxType::Eip1559, ..Default::default() },
            logs_bloom: Default::default(),
        }]]);

        let mut out = vec![];
        receipts.encode(&mut out);

        let mut out = out.as_slice();
        let decoded = Receipts::decode(&mut out).unwrap();

        assert_eq!(receipts, decoded);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_get_receipts() {
        let expected = hex!(
            "f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef"
        );
        let mut data = vec![];
        let request = RequestPair {
            request_id: 1111,
            message: GetReceipts(vec![
                hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                hex!("00000000000000000000000000000000000000000000000000000000feedbeef").into(),
            ]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_get_receipts() {
        let data = hex!(
            "f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef"
        );
        let request = RequestPair::<GetReceipts>::decode(&mut &data[..]).unwrap();
        assert_eq!(
            request,
            RequestPair {
                request_id: 1111,
                message: GetReceipts(vec![
                    hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                    hex!("00000000000000000000000000000000000000000000000000000000feedbeef").into(),
                ]),
            }
        );
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn encode_receipts() {
        let expected = hex!(
            "f90172820457f9016cf90169f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff"
        );
        let mut data = vec![];
        let request = RequestPair {
            request_id: 1111,
            message: Receipts(vec![vec![
                ReceiptWithBloom {
                    receipt: Receipt {
                        tx_type: TxType::Legacy,
                        cumulative_gas_used: 0x1u64,
                        logs: vec![
                            Log::new_unchecked(
                                hex!("0000000000000000000000000000000000000011").into(),
                                vec![
                                    hex!("000000000000000000000000000000000000000000000000000000000000dead").into(),
                                    hex!("000000000000000000000000000000000000000000000000000000000000beef").into(),
                                ],
                                hex!("0100ff")[..].into(),
                            ),
                        ],
                        success: false,
                    },
                    logs_bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                },
            ]]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn decode_receipts() {
        let data = hex!(
            "f90172820457f9016cf90169f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff"
        );
        let request = RequestPair::<Receipts>::decode(&mut &data[..]).unwrap();
        assert_eq!(
            request,
            RequestPair {
                request_id: 1111,
                message: Receipts(vec![
                    vec![
                        ReceiptWithBloom {
                            receipt: Receipt {
                                tx_type: TxType::Legacy,
                                cumulative_gas_used: 0x1u64,
                                logs: vec![
                                    Log::new_unchecked(
                                        hex!("0000000000000000000000000000000000000011").into(),
                                        vec![
                                            hex!("000000000000000000000000000000000000000000000000000000000000dead").into(),
                                            hex!("000000000000000000000000000000000000000000000000000000000000beef").into(),
                                        ],
                                        hex!("0100ff")[..].into(),
                                    ),
                                ],
                                success: false,
                            },
                            logs_bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                        },
                    ],
                ]),
            }
        );
    }

    #[test]
    fn decode_receipts_69() {
        let data = hex!("0xf9026605f90262f9025fc60201826590c0c7800183013cd9c0c702018301a2a5c0c7010183027a36c0c702018302e03ec0c7010183034646c0c702018303ac30c0c78001830483b8c0c702018304e9a2c0c780018305c17fc0c7020183062769c0c7800183068d71c0c702018306f35bc0c702018307cb77c0c701018308a382c0c7020183097ab6c0c78080830b0156c0c70101830b6740c0c70201830bcd48c0c70101830c32f6c0c70101830c98e0c0c70201830cfecac0c70201830d64b4c0c70280830dca9ec0c70101830e30a6c0c70201830f080dc0c70201830f6e15c0c78080830fd41dc0c702018310abbac0c701018310fdc2c0c7020183116370c0c780018311c95ac0c7010183122f44c0c701808312952ec0c7020183136c7dc0c70201831443c0c0c702018314a9c8c0c7020183150f94c0c7018083169634c0c7020183176d68c0c702808317d370c0c70201831838c4c0c701808319bf64c0c70201831a256cc0c78080831bac0cc0c70201831c11d8c0c70201831c77c2c0c78080831cdd34c0c70201831db57bc0c70101831e8d07c0c70101831ef2d3c0c70201831fcb37c0c70180832030e5c0c70201832096cfc0c701018320fcb9c0c70201832162c1c0c702018321c8abc0c7020183229ffac0c70201832305c6c0c7028083236bcec0c702808323d1d6c0c702018324a91cc0c7020183250f06c0c70201832574d2c0c7020183264c15c0c70201832723b6c0c70201832789a0c0c702018327ef8ac0c7020183285574c0c702018328bb40c0c702018329212ac0c7028083298714c0c70201832a5e4ec0c70201832ac438c0c70201832b9b72c0c70201832c017ac0");

        let request = RequestPair::<Receipts69>::decode(&mut &data[..]).unwrap();
        assert_eq!(
            request.message.0[0][0],
            Receipt {
                tx_type: TxType::Eip1559,
                success: true,
                cumulative_gas_used: 26000,
                logs: vec![],
            }
        );

        let encoded = alloy_rlp::encode(&request);
        assert_eq!(encoded, data);
    }

    #[test]
    fn encode_get_receipts70_inline_shape() {
        let req = RequestPair {
            request_id: 1111,
            message: GetReceipts70 {
                first_block_receipt_index: 0,
                block_hashes: vec![
                    hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                    hex!("00000000000000000000000000000000000000000000000000000000feedbeef").into(),
                ],
            },
        };

        let mut out = vec![];
        req.encode(&mut out);

        let mut buf = out.as_slice();
        let header = alloy_rlp::Header::decode(&mut buf).unwrap();
        let payload_start = buf.len();
        let request_id = u64::decode(&mut buf).unwrap();
        let first_block_receipt_index = u64::decode(&mut buf).unwrap();
        let block_hashes = Vec::<B256>::decode(&mut buf).unwrap();

        assert!(buf.is_empty(), "buffer not fully consumed");
        assert_eq!(request_id, 1111);
        assert_eq!(first_block_receipt_index, 0);
        assert_eq!(block_hashes.len(), 2);
        // ensure payload length matches header
        assert_eq!(payload_start - buf.len(), header.payload_length);

        let mut buf = out.as_slice();
        let decoded = RequestPair::<GetReceipts70>::decode(&mut buf).unwrap();
        assert!(buf.is_empty(), "buffer not fully consumed on decode");
        assert_eq!(decoded, req);
    }

    #[test]
    fn encode_receipts70_inline_shape() {
        let payload: Receipts70<Receipt> =
            Receipts70 { last_block_incomplete: true, receipts: vec![vec![Receipt::default()]] };

        let resp = RequestPair { request_id: 7, message: payload };

        let mut out = vec![];
        resp.encode(&mut out);

        let mut buf = out.as_slice();
        let header = alloy_rlp::Header::decode(&mut buf).unwrap();
        let payload_start = buf.len();
        let request_id = u64::decode(&mut buf).unwrap();
        let last_block_incomplete = bool::decode(&mut buf).unwrap();
        let receipts = Vec::<Vec<Receipt>>::decode(&mut buf).unwrap();

        assert!(buf.is_empty(), "buffer not fully consumed");
        assert_eq!(payload_start - buf.len(), header.payload_length);
        assert_eq!(request_id, 7);
        assert!(last_block_incomplete);
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].len(), 1);

        let mut buf = out.as_slice();
        let decoded = RequestPair::<Receipts70>::decode(&mut buf).unwrap();
        assert!(buf.is_empty(), "buffer not fully consumed on decode");
        assert_eq!(decoded, resp);
    }
}
