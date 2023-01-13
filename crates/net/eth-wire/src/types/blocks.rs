//! Implements the `GetBlockHeaders`, `GetBlockBodies`, `BlockHeaders`, and `BlockBodies` message
//! types.
use super::RawBlockBody;
use reth_primitives::{BlockHashOrNumber, Header, HeadersDirection, TransactionSigned, H256};
use reth_rlp::{RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper};
use serde::{Deserialize, Serialize};

/// A request for a peer to return block headers starting at the requested block.
/// The peer must return at most [`limit`](#structfield.limit) headers.
/// If the [`reverse`](#structfield.reverse) field is `true`, the headers will be returned starting
/// at [`start_block`](#structfield.start_block), traversing towards the genesis block.
/// Otherwise, headers will be returned starting at [`start_block`](#structfield.start_block),
/// traversing towards the latest block.
///
/// If the [`skip`](#structfield.skip) field is non-zero, the peer must skip that amount of headers
/// in the direction specified by [`reverse`](#structfield.reverse).
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, Hash, RlpEncodable, RlpDecodable, Serialize, Deserialize,
)]
pub struct GetBlockHeaders {
    /// The block number or hash that the peer should start returning headers from.
    pub start_block: BlockHashOrNumber,

    /// The maximum number of headers to return.
    pub limit: u64,

    /// The number of blocks that the node should skip while traversing and returning headers.
    /// A skip value of zero denotes that the peer should return contiguous heaaders, starting from
    /// [`start_block`](#structfield.start_block) and returning at most
    /// [`limit`](#structfield.limit) headers.
    pub skip: u32,

    /// The direction in which the headers should be returned in.
    pub direction: HeadersDirection,
}

/// The response to [`GetBlockHeaders`], containing headers if any headers were found.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    Serialize,
    Deserialize,
    Default,
)]
pub struct BlockHeaders(
    /// The requested headers.
    pub Vec<Header>,
);

impl From<Vec<Header>> for BlockHeaders {
    fn from(headers: Vec<Header>) -> Self {
        BlockHeaders(headers)
    }
}

/// A request for a peer to return block bodies for the given block hashes.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    Serialize,
    Deserialize,
    Default,
)]
pub struct GetBlockBodies(
    /// The block hashes to request bodies for.
    pub Vec<H256>,
);

impl From<Vec<H256>> for GetBlockBodies {
    fn from(hashes: Vec<H256>) -> Self {
        GetBlockBodies(hashes)
    }
}

// TODO(onbjerg): We should have this type in primitives
/// A response to [`GetBlockBodies`], containing bodies if any bodies were found.
#[derive(
    Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Serialize, Deserialize, Default,
)]
pub struct BlockBody {
    /// Transactions in the block
    pub transactions: Vec<TransactionSigned>,
    /// Uncle headers for the given block
    pub ommers: Vec<Header>,
}

impl BlockBody {
    /// Create a [`Block`] from the body and its header.
    pub fn create_block(&self, header: &Header) -> RawBlockBody {
        RawBlockBody {
            header: header.clone(),
            transactions: self.transactions.clone(),
            ommers: self.ommers.clone(),
        }
    }
}

/// The response to [`GetBlockBodies`], containing the block bodies that the peer knows about if
/// any were found.
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    RlpEncodableWrapper,
    RlpDecodableWrapper,
    Serialize,
    Deserialize,
    Default,
)]
pub struct BlockBodies(
    /// The requested block bodies, each of which should correspond to a hash in the request.
    pub Vec<BlockBody>,
);

impl From<Vec<BlockBody>> for BlockBodies {
    fn from(bodies: Vec<BlockBody>) -> Self {
        BlockBodies(bodies)
    }
}

#[cfg(test)]
mod test {
    use crate::types::{
        message::RequestPair, BlockBodies, BlockHeaders, GetBlockBodies, GetBlockHeaders,
    };
    use hex_literal::hex;
    use reth_primitives::{
        BlockHashOrNumber, Header, HeadersDirection, Signature, Transaction, TransactionKind,
        TransactionSigned, TxLegacy, U256,
    };
    use reth_rlp::{Decodable, Encodable};
    use std::str::FromStr;

    use super::BlockBody;

    #[test]
    fn decode_hash() {
        // this is a valid 32 byte rlp string
        let rlp = hex!("a0ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
        let decoded_number = BlockHashOrNumber::decode(&mut &rlp[..]).unwrap();
        let full_bytes = [0xff; 32].into();
        let expected = BlockHashOrNumber::Hash(full_bytes);
        assert_eq!(expected, decoded_number);
    }

    #[test]
    fn decode_number() {
        // this is a valid 64 bit number
        let rlp = hex!("88ffffffffffffffff");
        let decoded_number = BlockHashOrNumber::decode(&mut &rlp[..]).unwrap();
        let expected = BlockHashOrNumber::Number(u64::MAX);
        assert_eq!(expected, decoded_number);
    }

    #[test]
    fn decode_largest_single_byte() {
        // the largest single byte is 0x7f, so we should be able to decode this into a u64
        let rlp = hex!("7f");
        let decoded_number = BlockHashOrNumber::decode(&mut &rlp[..]).unwrap();
        let expected = BlockHashOrNumber::Number(0x7fu64);
        assert_eq!(expected, decoded_number);
    }

    #[test]
    fn decode_long_hash() {
        // let's try a 33 byte long string
        // 0xa1 = 0x80 (start of string) + 0x21 (33, length of string)
        let long_rlp = hex!("a1ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff");
        let decode_result = BlockHashOrNumber::decode(&mut &long_rlp[..]);
        if decode_result.is_ok() {
            panic!("Decoding a bytestring longer than 32 bytes should not decode successfully");
        }
    }

    #[test]
    fn decode_long_number() {
        // let's try a 72 bit number
        // 0x89 = 0x80 (start of string) + 0x09 (9, length of string)
        let long_number = hex!("89ffffffffffffffffff");
        let decode_result = BlockHashOrNumber::decode(&mut &long_number[..]);
        if decode_result.is_ok() {
            panic!("Decoding a number longer than 64 bits (but not exactly 32 bytes) should not decode successfully");
        }
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_get_block_header() {
        let expected = hex!(
            "e8820457e4a000000000000000000000000000000000000000000000000000000000deadc0de050580"
        );
        let mut data = vec![];
        RequestPair::<GetBlockHeaders> {
            request_id: 1111,
            message: GetBlockHeaders {
                start_block: BlockHashOrNumber::Hash(
                    hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                ),
                limit: 5,
                skip: 5,
                direction: HeadersDirection::Rising,
            },
        }
        .encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_get_block_header() {
        let data = hex!(
            "e8820457e4a000000000000000000000000000000000000000000000000000000000deadc0de050580"
        );
        let expected = RequestPair::<GetBlockHeaders> {
            request_id: 1111,
            message: GetBlockHeaders {
                start_block: BlockHashOrNumber::Hash(
                    hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                ),
                limit: 5,
                skip: 5,
                direction: HeadersDirection::Rising,
            },
        };
        let result = RequestPair::decode(&mut &data[..]);
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_get_block_header_number() {
        let expected = hex!("ca820457c682270f050580");
        let mut data = vec![];
        RequestPair::<GetBlockHeaders> {
            request_id: 1111,
            message: GetBlockHeaders {
                start_block: BlockHashOrNumber::Number(9999),
                limit: 5,
                skip: 5,
                direction: HeadersDirection::Rising,
            },
        }
        .encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_get_block_header_number() {
        let data = hex!("ca820457c682270f050580");
        let expected = RequestPair::<GetBlockHeaders> {
            request_id: 1111,
            message: GetBlockHeaders {
                start_block: BlockHashOrNumber::Number(9999),
                limit: 5,
                skip: 5,
                direction: HeadersDirection::Rising,
            },
        };
        let result = RequestPair::decode(&mut &data[..]);
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_block_header() {
        // [ (f90202) 0x0457 = 1111, [ (f901fc) [ (f901f9) header ] ] ]
        let expected = hex!("f90202820457f901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000");
        let mut data = vec![];
        RequestPair::<BlockHeaders> {
            request_id: 1111,
            message: BlockHeaders(vec![
                Header {
                    parent_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    ommers_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    beneficiary: hex!("0000000000000000000000000000000000000000").into(),
                    state_root: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    transactions_root: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    receipts_root: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    logs_bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                    difficulty: U256::from(0x8aeu64),
                    number: 0xd05u64,
                    gas_limit: 0x115cu64,
                    gas_used: 0x15b3u64,
                    timestamp: 0x1a0au64,
                    extra_data: hex!("7788")[..].into(),
                    mix_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    nonce: 0x0000000000000000u64,
                    base_fee_per_gas: None,
                },
            ]),
        }.encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_block_header() {
        let data = hex!("f90202820457f901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000");
        let expected = RequestPair::<BlockHeaders> {
            request_id: 1111,
            message: BlockHeaders(vec![
                Header {
                    parent_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    ommers_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    beneficiary: hex!("0000000000000000000000000000000000000000").into(),
                    state_root: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    transactions_root: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    receipts_root: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    logs_bloom: hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000").into(),
                    difficulty: U256::from(0x8aeu64),
                    number: 0xd05u64,
                    gas_limit: 0x115cu64,
                    gas_used: 0x15b3u64,
                    timestamp: 0x1a0au64,
                    extra_data: hex!("7788")[..].into(),
                    mix_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    nonce: 0x0000000000000000u64,
                    base_fee_per_gas: None,
                },
            ]),
        };
        let result = RequestPair::decode(&mut &data[..]);
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_get_block_bodies() {
        let expected = hex!("f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef");
        let mut data = vec![];
        RequestPair::<GetBlockBodies> {
            request_id: 1111,
            message: GetBlockBodies(vec![
                hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                hex!("00000000000000000000000000000000000000000000000000000000feedbeef").into(),
            ]),
        }
        .encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_get_block_bodies() {
        let data = hex!("f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef");
        let expected = RequestPair::<GetBlockBodies> {
            request_id: 1111,
            message: GetBlockBodies(vec![
                hex!("00000000000000000000000000000000000000000000000000000000deadc0de").into(),
                hex!("00000000000000000000000000000000000000000000000000000000feedbeef").into(),
            ]),
        };
        let result = RequestPair::decode(&mut &data[..]);
        assert_eq!(result.unwrap(), expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn encode_block_bodies() {
        let expected =
    hex!("f902dc820457f902d6f902d3f8d2f867088504a817c8088302e2489435353535353535353535353535353535353535358202008025a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10f867098504a817c809830334509435353535353535353535353535353535353535358202d98025a052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afba052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afbf901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000"
    );
        let mut data = vec![];
        let request = RequestPair::<BlockBodies> {
            request_id: 1111,
            message: BlockBodies(vec![
                BlockBody {
                    transactions: vec![
                        TransactionSigned::from_transaction_and_signature(Transaction::Legacy(TxLegacy {
                            chain_id: Some(1),
                            nonce: 0x8u64,
                            gas_price: 0x4a817c808,
                            gas_limit: 0x2e248u64,
                            to:
    TransactionKind::Call(hex!("3535353535353535353535353535353535353535").into()),
                            value: 0x200u64.into(),
                            input: Default::default(),
                        }),
                        Signature {
                                odd_y_parity: false,
                                r:
    U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12").unwrap(),
                                s:
    U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10").unwrap(),
                            }
                        ),
                        TransactionSigned::from_transaction_and_signature(Transaction::Legacy(TxLegacy {
                            chain_id: Some(1),
                            nonce: 0x9u64,
                            gas_price: 0x4a817c809,
                            gas_limit: 0x33450u64,
                            to:
    TransactionKind::Call(hex!("3535353535353535353535353535353535353535").into()),
                            value: 0x2d9u64.into(),
                            input: Default::default(),
                        }), Signature {
                                odd_y_parity: false,
                                r:
    U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                s:
    U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                            },
                        ),
                    ],
                    ommers: vec![
                        Header {
                            parent_hash:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            ommers_hash:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            beneficiary: hex!("0000000000000000000000000000000000000000").into(),
                            state_root:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            transactions_root:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            receipts_root:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            logs_bloom:
    hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    ).into(),                         difficulty: U256::from(0x8aeu64),
                            number: 0xd05u64,
                            gas_limit: 0x115cu64,
                            gas_used: 0x15b3u64,
                            timestamp: 0x1a0au64,
                            extra_data: hex!("7788")[..].into(),
                            mix_hash:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            nonce: 0x0000000000000000u64,
                            base_fee_per_gas: None,
                        },
                    ],
                }
            ]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    #[test]
    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    fn decode_block_bodies() {
        let data =
    hex!("f902dc820457f902d6f902d3f8d2f867088504a817c8088302e2489435353535353535353535353535353535353535358202008025a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10f867098504a817c809830334509435353535353535353535353535353535353535358202d98025a052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afba052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afbf901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000"
    );
        let expected = RequestPair::<BlockBodies> {
            request_id: 1111,
            message: BlockBodies(vec![
                BlockBody {
                    transactions: vec![
                        TransactionSigned::from_transaction_and_signature(Transaction::Legacy(TxLegacy {
                            chain_id: Some(1),
                            nonce: 0x8u64,
                            gas_price: 0x4a817c808,
                            gas_limit: 0x2e248u64,
                            to:
    TransactionKind::Call(hex!("3535353535353535353535353535353535353535").into()),
                            value: 0x200u64.into(),
                            input: Default::default(),
                        }),
                        Signature {
                                odd_y_parity: false,
                                r:
    U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12").unwrap(),
                                s:
    U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10").unwrap(),
                            }
                        ),
                        TransactionSigned::from_transaction_and_signature(Transaction::Legacy(TxLegacy {
                            chain_id: Some(1),
                            nonce: 0x9u64,
                            gas_price: 0x4a817c809,
                            gas_limit: 0x33450u64,
                            to:
    TransactionKind::Call(hex!("3535353535353535353535353535353535353535").into()),
                            value: 0x2d9u64.into(),
                            input: Default::default(),
                        }),
                        Signature {
                                odd_y_parity: false,
                                r:
    U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                s:
    U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                            },
                        ),
                    ],
                    ommers: vec![
                        Header {
                            parent_hash:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            ommers_hash:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            beneficiary: hex!("0000000000000000000000000000000000000000").into(),
                            state_root:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            transactions_root:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            receipts_root:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            logs_bloom:
    hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"
    ).into(),                         difficulty: U256::from(0x8aeu64),
                            number: 0xd05u64,
                            gas_limit: 0x115cu64,
                            gas_used: 0x15b3u64,
                            timestamp: 0x1a0au64,
                            extra_data: hex!("7788")[..].into(),
                            mix_hash:
    hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            nonce: 0x0000000000000000u64,
                            base_fee_per_gas: None,
                        },
                    ],
                }
            ]),
        };
        let result = RequestPair::decode(&mut &data[..]).unwrap();
        assert_eq!(result, expected);
    }
}
