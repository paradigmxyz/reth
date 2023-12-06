//! Implements the `GetReceipts` and `Receipts` message types.
use alloy_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
use reth_codecs::derive_arbitrary;
use reth_primitives::{ReceiptWithBloom, B256};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A request for transaction receipts from the given block hashes.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GetReceipts(
    /// The block hashes to request receipts for.
    pub Vec<B256>,
);

/// The response to [`GetReceipts`], containing receipt lists that correspond to each block
/// requested.
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Receipts(
    /// Each receipt hash should correspond to a block hash in the request.
    #[cfg_attr(
        any(test, feature = "arbitrary"),
        proptest(
            strategy = "proptest::collection::vec(proptest::collection::vec(proptest::arbitrary::any::<ReceiptWithBloom>(), 0..=50), 0..=5)"
        )
    )]
    pub Vec<Vec<ReceiptWithBloom>>,
);

#[cfg(test)]
mod test {
    use crate::{
        types::{message::RequestPair, GetReceipts},
        Receipts,
    };
    use alloy_rlp::{Decodable, Encodable};
    use reth_primitives::{hex, Log, Receipt, ReceiptWithBloom, TxType};

    #[test]
    fn roundtrip_eip1559() {
        let receipts = Receipts(vec![vec![ReceiptWithBloom {
            receipt: Receipt {
                tx_type: TxType::EIP1559,
                success: false,
                cumulative_gas_used: 0,
                logs: vec![],
                #[cfg(feature = "optimism")]
                deposit_nonce: None,
                #[cfg(feature = "optimism")]
                deposit_receipt_version: None,
            },
            bloom: Default::default(),
        }]]);

        let mut out = vec![];
        receipts.encode(&mut out);

        let mut out = out.as_slice();
        let decoded = Receipts::decode(&mut out).unwrap();

        assert!(receipts == decoded);
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
                ])
            }
        );
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
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
                            Log {
                                address: hex!("0000000000000000000000000000000000000011").into(),
                                topics: vec![
                                    hex!("000000000000000000000000000000000000000000000000000000000000dead").into(),
                                    hex!("000000000000000000000000000000000000000000000000000000000000beef").into(),
                                ],
                                data: hex!("0100ff")[..].into(),
                            },
                        ],
                        success: false,
                        #[cfg(feature = "optimism")]
                        deposit_nonce: None,
                        #[cfg(feature = "optimism")]
                        deposit_receipt_version: None,
                    },
                    bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                },
            ]]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
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
                                    Log {
                                        address: hex!("0000000000000000000000000000000000000011").into(),
                                        topics: vec![
                                            hex!("000000000000000000000000000000000000000000000000000000000000dead").into(),
                                            hex!("000000000000000000000000000000000000000000000000000000000000beef").into(),
                                        ],
                                        data: hex!("0100ff")[..].into(),
                                    },
                                ],
                                success: false,
                                #[cfg(feature = "optimism")]
                                deposit_nonce: None,
                                #[cfg(feature = "optimism")]
                                deposit_receipt_version: None,
                            },
                            bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                        },
                    ],
                ]),
            }
        );
    }
}
