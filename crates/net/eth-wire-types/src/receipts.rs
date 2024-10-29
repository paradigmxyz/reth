//! Implements the `GetReceipts` and `Receipts` message types.

use alloy_primitives::B256;
use alloy_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
use reth_codecs_derive::add_arbitrary_tests;
use reth_primitives::ReceiptWithBloom;

/// A request for transaction receipts from the given block hashes.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetReceipts(
    /// The block hashes to request receipts for.
    pub Vec<B256>,
);

/// The response to [`GetReceipts`], containing receipt lists that correspond to each block
/// requested.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct Receipts(
    /// Each receipt hash should correspond to a block hash in the request.
    pub Vec<Vec<ReceiptWithBloom>>,
);

#[cfg(test)]
mod tests {
    use crate::{message::RequestPair, GetReceipts, Receipts};
    use alloy_primitives::{hex, Log};
    use alloy_rlp::{Decodable, Encodable};
    use reth_primitives::{Receipt, ReceiptWithBloom, TxType};

    #[test]
    fn roundtrip_eip1559() {
        let receipts = Receipts(vec![vec![ReceiptWithBloom {
            receipt: Receipt { tx_type: TxType::Eip1559, ..Default::default() },
            bloom: Default::default(),
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
        let expected = hex!("f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef");
        let mut data = vec![];
        let request = RequestPair::<GetReceipts> {
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
        let data = hex!("f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef");
        let request = RequestPair::<GetReceipts>::decode(&mut &data[..]).unwrap();
        assert_eq!(
            request,
            RequestPair::<GetReceipts> {
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
    #[allow(clippy::needless_update)]
    fn encode_receipts() {
        let expected = hex!("f90172820457f9016cf90169f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff");
        let mut data = vec![];
        let request = RequestPair::<Receipts> {
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
                        ..Default::default()
                    },
                    bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                },
            ]]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    #[allow(clippy::needless_update)]
    fn decode_receipts() {
        let data = hex!("f90172820457f9016cf90169f901668001b9010000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000f85ff85d940000000000000000000000000000000000000011f842a0000000000000000000000000000000000000000000000000000000000000deada0000000000000000000000000000000000000000000000000000000000000beef830100ff");
        let request = RequestPair::<Receipts>::decode(&mut &data[..]).unwrap();
        assert_eq!(
            request,
            RequestPair::<Receipts> {
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
                                ..Default::default()
                            },
                            bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                        },
                    ],
                ]),
            }
        );
    }
}
