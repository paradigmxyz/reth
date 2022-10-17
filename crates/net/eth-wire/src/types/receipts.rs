use anvil_core::eth::receipt::TypedReceipt;
use fastrlp::{RlpDecodableWrapper, RlpEncodableWrapper};

/// A request for transaction receipts from the given block hashes.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper)]
pub struct GetReceipts(
    /// The block hashes to request receipts for.
    pub Vec<[u8; 32]>,
);

/// The response to [`GetReceipts`], containing receipt lists that correspond to each block
/// requested.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper)]
pub struct Receipts(
    /// Each receipt hash should correspond to a block hash in the request.
    pub Vec<Vec<TypedReceipt>>,
);

#[cfg(test)]
mod test {
    use anvil_core::eth::receipt::{EIP658Receipt, Log, TypedReceipt};
    use hex_literal::hex;

    use crate::{message::RequestPair, GetReceipts, Receipts};
    use fastrlp::{Decodable, Encodable};

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_get_receipts() {
        let expected = hex!("f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef");
        let mut data = vec![];
        let request = RequestPair::<GetReceipts> {
            request_id: 1111,
            message: GetReceipts(vec![
                hex!("00000000000000000000000000000000000000000000000000000000deadc0de"),
                hex!("00000000000000000000000000000000000000000000000000000000feedbeef"),
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
                    hex!("00000000000000000000000000000000000000000000000000000000deadc0de"),
                    hex!("00000000000000000000000000000000000000000000000000000000feedbeef"),
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
                    TypedReceipt::Legacy( EIP658Receipt {
                        logs_bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                        gas_used: 0x1u64.into(),
                        logs: vec![
                            Log {
                                address: hex!("0000000000000000000000000000000000000011").into(),
                                topics: vec![
                                    hex!("000000000000000000000000000000000000000000000000000000000000dead").into(),
                                    hex!("000000000000000000000000000000000000000000000000000000000000beef").into(),
                                ],
                                data: hex!("0100ff").into(),
                            },
                        ],
                        status_code: 0,
                    }),
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
                        TypedReceipt::Legacy( EIP658Receipt {
                            logs_bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                            gas_used: 0x1u64.into(),
                            logs: vec![
                                Log {
                                    address: hex!("0000000000000000000000000000000000000011").into(),
                                    topics: vec![
                                        hex!("000000000000000000000000000000000000000000000000000000000000dead").into(),
                                        hex!("000000000000000000000000000000000000000000000000000000000000beef").into(),
                                    ],
                                    data: hex!("0100ff").into(),
                                },
                            ],
                            status_code: 0,
                        }),
                    ],
                ]),
            }
        );
    }
}
