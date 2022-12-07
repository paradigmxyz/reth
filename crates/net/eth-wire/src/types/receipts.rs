//! Implements the `GetReceipts` and `Receipts` message types.
use reth_primitives::{Receipt, H256};
use reth_rlp::{RlpDecodableWrapper, RlpEncodableWrapper};
use serde::{Deserialize, Serialize};

/// A request for transaction receipts from the given block hashes.
#[derive(
    Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Serialize, Deserialize,
)]
pub struct GetReceipts(
    /// The block hashes to request receipts for.
    pub Vec<H256>,
);

/// The response to [`GetReceipts`], containing receipt lists that correspond to each block
/// requested.
#[derive(
    Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Serialize, Deserialize,
)]
pub struct Receipts(
    /// Each receipt hash should correspond to a block hash in the request.
    pub Vec<Vec<Receipt>>,
);

#[cfg(test)]
mod test {
    use crate::types::{message::RequestPair, GetReceipts, Receipts};
    use hex_literal::hex;
    use reth_primitives::{Log, Receipt, TxType};
    use reth_rlp::{Decodable, Encodable};

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
            message: Receipts(vec![
                vec![
                    Receipt {
                        tx_type: TxType::Legacy,
                        bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
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
                    },
                ],
            ]),
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
                        Receipt {
                            tx_type: TxType::Legacy,
                            bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
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
                        },
                    ],
                ]),
            }
        );
    }
}
