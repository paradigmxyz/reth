//! Implements the `GetBlockHeaders`, `GetBlockBodies`, `BlockHeaders`, and `BlockBodies` message
//! types.

use crate::HeadersDirection;
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::B256;
use alloy_rlp::{RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper};
use reth_codecs_derive::add_arbitrary_tests;
use reth_primitives::{BlockBody, Header};

/// A request for a peer to return block headers starting at the requested block.
/// The peer must return at most [`limit`](#structfield.limit) headers.
/// If the [`reverse`](#structfield.reverse) field is `true`, the headers will be returned starting
/// at [`start_block`](#structfield.start_block), traversing towards the genesis block.
/// Otherwise, headers will be returned starting at [`start_block`](#structfield.start_block),
/// traversing towards the latest block.
///
/// If the [`skip`](#structfield.skip) field is non-zero, the peer must skip that amount of headers
/// in the direction specified by [`reverse`](#structfield.reverse).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, RlpEncodable, RlpDecodable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetBlockHeaders {
    /// The block number or hash that the peer should start returning headers from.
    pub start_block: BlockHashOrNumber,

    /// The maximum number of headers to return.
    pub limit: u64,

    /// The number of blocks that the node should skip while traversing and returning headers.
    /// A skip value of zero denotes that the peer should return contiguous headers, starting from
    /// [`start_block`](#structfield.start_block) and returning at most
    /// [`limit`](#structfield.limit) headers.
    pub skip: u32,

    /// The direction in which the headers should be returned in.
    pub direction: HeadersDirection,
}

/// The response to [`GetBlockHeaders`], containing headers if any headers were found.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[add_arbitrary_tests(rlp, 10)]
pub struct BlockHeaders(
    /// The requested headers.
    pub Vec<Header>,
);

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for BlockHeaders {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let headers_count: usize = u.int_in_range(0..=10)?;
        let mut headers = Vec::with_capacity(headers_count);

        for _ in 0..headers_count {
            headers.push(reth_primitives::generate_valid_header(
                u.arbitrary()?,
                u.arbitrary()?,
                u.arbitrary()?,
                u.arbitrary()?,
                u.arbitrary()?,
            ))
        }

        Ok(Self(headers))
    }
}

impl From<Vec<Header>> for BlockHeaders {
    fn from(headers: Vec<Header>) -> Self {
        Self(headers)
    }
}

/// A request for a peer to return block bodies for the given block hashes.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetBlockBodies(
    /// The block hashes to request bodies for.
    pub Vec<B256>,
);

impl From<Vec<B256>> for GetBlockBodies {
    fn from(hashes: Vec<B256>) -> Self {
        Self(hashes)
    }
}

/// The response to [`GetBlockBodies`], containing the block bodies that the peer knows about if
/// any were found.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp, 16)]
pub struct BlockBodies(
    /// The requested block bodies, each of which should correspond to a hash in the request.
    pub Vec<BlockBody>,
);

impl From<Vec<BlockBody>> for BlockBodies {
    fn from(bodies: Vec<BlockBody>) -> Self {
        Self(bodies)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        message::RequestPair, BlockBodies, BlockHeaders, GetBlockBodies, GetBlockHeaders,
        HeadersDirection,
    };
    use alloy_consensus::TxLegacy;
    use alloy_primitives::{hex, Parity, TxKind, U256};
    use alloy_rlp::{Decodable, Encodable};
    use reth_primitives::{BlockHashOrNumber, Header, Signature, Transaction, TransactionSigned};
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
        assert!(
            decode_result.is_err(),
            "Decoding a bytestring longer than 32 bytes should not decode successfully"
        );
    }

    #[test]
    fn decode_long_number() {
        // let's try a 72 bit number
        // 0x89 = 0x80 (start of string) + 0x09 (9, length of string)
        let long_number = hex!("89ffffffffffffffffff");
        let decode_result = BlockHashOrNumber::decode(&mut &long_number[..]);
        assert!(decode_result.is_err(), "Decoding a number longer than 64 bits (but not exactly 32 bytes) should not decode successfully");
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
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

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
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

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
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

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
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

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
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
                    gas_limit: 0x115c,
                    gas_used: 0x15b3,
                    timestamp: 0x1a0au64,
                    extra_data: hex!("7788")[..].into(),
                    mix_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    nonce: 0x0000000000000000u64.into(),
                    base_fee_per_gas: None,
                    withdrawals_root: None,
                    blob_gas_used: None,
                    excess_blob_gas: None,
                    parent_beacon_block_root: None,
                    requests_root: None
                },
            ]),
        }.encode(&mut data);
        assert_eq!(data, expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
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
                    gas_limit: 0x115c,
                    gas_used: 0x15b3,
                    timestamp: 0x1a0au64,
                    extra_data: hex!("7788")[..].into(),
                    mix_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                    nonce: 0x0000000000000000u64.into(),
                    base_fee_per_gas: None,
                    withdrawals_root: None,
                    blob_gas_used: None,
                    excess_blob_gas: None,
                    parent_beacon_block_root: None,
                    requests_root: None
                },
            ]),
        };
        let result = RequestPair::decode(&mut &data[..]);
        assert_eq!(result.unwrap(), expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
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

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
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

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn encode_block_bodies() {
        let expected = hex!("f902dc820457f902d6f902d3f8d2f867088504a817c8088302e2489435353535353535353535353535353535353535358202008025a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10f867098504a817c809830334509435353535353535353535353535353535353535358202d98025a052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afba052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afbf901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000");
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
                            gas_limit: 0x2e248,
                            to: TxKind::Call(hex!("3535353535353535353535353535353535353535").into()),
                            value: U256::from(0x200u64),
                            input: Default::default(),
                        }), Signature::new(
                                U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12").unwrap(),
                                U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10").unwrap(),
                                Parity::Parity(false),
                            ),
                        ),
                        TransactionSigned::from_transaction_and_signature(Transaction::Legacy(TxLegacy {
                            chain_id: Some(1),
                            nonce: 0x9u64,
                            gas_price: 0x4a817c809,
                            gas_limit: 0x33450,
                            to: TxKind::Call(hex!("3535353535353535353535353535353535353535").into()),
                            value: U256::from(0x2d9u64),
                            input: Default::default(),
                        }), Signature::new(
                                U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                Parity::Parity(false),
                            ),
                        ),
                    ],
                    ommers: vec![
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
                            gas_limit: 0x115c,
                            gas_used: 0x15b3,
                            timestamp: 0x1a0au64,
                            extra_data: hex!("7788")[..].into(),
                            mix_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            nonce: 0x0000000000000000u64.into(),
                            base_fee_per_gas: None,
                            withdrawals_root: None,
                            blob_gas_used: None,
                            excess_blob_gas: None,
                            parent_beacon_block_root: None,
                            requests_root: None
                        },
                    ],
                    withdrawals: None,
                    requests: None
                }
            ]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn decode_block_bodies() {
        let data = hex!("f902dc820457f902d6f902d3f8d2f867088504a817c8088302e2489435353535353535353535353535353535353535358202008025a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10f867098504a817c809830334509435353535353535353535353535353535353535358202d98025a052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afba052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afbf901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000");
        let expected = RequestPair::<BlockBodies> {
            request_id: 1111,
            message: BlockBodies(vec![
                BlockBody {
                    transactions: vec![
                        TransactionSigned::from_transaction_and_signature(Transaction::Legacy(
                            TxLegacy {
                                chain_id: Some(1),
                                nonce: 0x8u64,
                                gas_price: 0x4a817c808,
                                gas_limit: 0x2e248,
                                to: TxKind::Call(hex!("3535353535353535353535353535353535353535").into()),
                                value: U256::from(0x200u64),
                                input: Default::default(),
                            }),
                            Signature::new(
                                U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12").unwrap(),
                                U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10").unwrap(),
                                Parity::Eip155(37),
                            ),
                        ),
                        TransactionSigned::from_transaction_and_signature(
                            Transaction::Legacy(TxLegacy {
                                chain_id: Some(1),
                                nonce: 0x9u64,
                                gas_price: 0x4a817c809,
                                gas_limit: 0x33450,
                                to: TxKind::Call(hex!("3535353535353535353535353535353535353535").into()),
                                value: U256::from(0x2d9u64),
                                input: Default::default(),
                            }),
                            Signature::new(
                                U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                Parity::Eip155(37),
                            ),
                        ),
                    ],
                    ommers: vec![
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
                            gas_limit: 0x115c,
                            gas_used: 0x15b3,
                            timestamp: 0x1a0au64,
                            extra_data: hex!("7788")[..].into(),
                            mix_hash: hex!("0000000000000000000000000000000000000000000000000000000000000000").into(),
                            nonce: 0x0000000000000000u64.into(),
                            base_fee_per_gas: None,
                            withdrawals_root: None,
                            blob_gas_used: None,
                            excess_blob_gas: None,
                            parent_beacon_block_root: None,
                            requests_root: None
                        },
                    ],
                    withdrawals: None,
                    requests: None
                }
            ]),
        };
        let result = RequestPair::decode(&mut &data[..]).unwrap();
        assert_eq!(result, expected);
    }
}
