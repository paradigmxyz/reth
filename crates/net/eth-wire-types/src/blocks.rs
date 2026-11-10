//! Implements the `GetBlockHeaders`, `GetBlockBodies`, `BlockHeaders`, and `BlockBodies` message
//! types.

use crate::{
    message::{decoded_capacity, decoded_list_size, ensure_list_within_memory_budget},
    HeadersDirection,
};
use alloc::vec::Vec;
use alloy_eips::{eip4895::Withdrawal, BlockHashOrNumber};
use alloy_primitives::B256;
use alloy_rlp::{
    Decodable, Header, RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper,
};
use derive_more::{Deref, IntoIterator};
use reth_codecs_derive::{add_arbitrary_tests, generate_tests};
use reth_primitives_traits::BlockBody;

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
pub struct BlockHeaders<H = alloy_consensus::Header>(
    /// The requested headers.
    pub Vec<H>,
);

generate_tests!(#[rlp, 10] BlockHeaders<alloy_consensus::Header>, EthBlockHeadersTests);

impl<H> From<Vec<H>> for BlockHeaders<H> {
    fn from(headers: Vec<H>) -> Self {
        Self(headers)
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
    Default,
    Deref,
    IntoIterator,
)]
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
pub struct BlockBodies<B = reth_ethereum_primitives::BlockBody>(
    /// The requested block bodies, each of which should correspond to a hash in the request.
    pub Vec<B>,
);

generate_tests!(#[rlp, 16] BlockBodies<reth_ethereum_primitives::BlockBody>, EthBlockBodiesTests);

impl<B> From<Vec<B>> for BlockBodies<B> {
    fn from(bodies: Vec<B>) -> Self {
        Self(bodies)
    }
}

impl<B: BlockBody> BlockBodies<B> {
    /// Decodes the RLP list of block bodies, rejecting it before decoding if the bodies with their
    /// transactions, ommers and withdrawals would take more than `memory_budget` bytes in memory.
    ///
    /// Each list is counted at the size of its items' type, for the capacity that the decoded
    /// `Vec` grows to.
    pub fn decode_with_memory_budget(
        buf: &mut &[u8],
        memory_budget: usize,
    ) -> alloy_rlp::Result<Self> {
        ensure_list_within_memory_budget::<B>(buf, memory_budget, |buf| {
            // A block body is encoded as `[transactions, ommers, withdrawals]`, where withdrawals
            // are omitted before Shanghai.
            let mut body = Header::decode_bytes(buf, true)?;
            let mut size = decoded_transaction_list_size::<B::Transaction>(&mut body)?;
            if !body.is_empty() {
                size = size.saturating_add(decoded_list_size::<B::OmmerHeader>(&mut body)?);
            }
            if !body.is_empty() {
                size = size.saturating_add(decoded_list_size::<Withdrawal>(&mut body)?);
            }
            Ok(size)
        })?;
        Self::decode(buf)
    }
}

/// Returns the bytes that decoding the RLP list of transactions at the front of `buf` into a
/// `Vec<T>` takes, without decoding them, and advances `buf` past the list.
///
/// Transactions are counted the way the EIP-2718 network decoder reads them: a legacy transaction
/// is an RLP list, and a typed transaction is its type byte followed by the RLP list of its fields.
/// The decoder does not hold a typed transaction to the length of the RLP string around it, so it
/// reads bytes left in that string as further transactions, and so does this count.
fn decoded_transaction_list_size<T>(buf: &mut &[u8]) -> alloy_rlp::Result<usize> {
    let mut txs = Header::decode_bytes(buf, true)?;
    let mut len = 0usize;
    while !txs.is_empty() {
        let header = Header::decode(&mut txs)?;
        if header.list {
            // `Header::decode` checked that the payload is in bounds.
            txs = &txs[header.payload_length..];
        } else {
            // `Header::decode` steps over a string header but not over a bare type byte, so the
            // type byte is next either way.
            txs = txs.get(1..).ok_or(alloy_rlp::Error::InputTooShort)?;
            Header::decode_bytes(&mut txs, true)?;
        }
        len += 1;
    }
    Ok(decoded_capacity(len).saturating_mul(core::mem::size_of::<T>()))
}

#[cfg(test)]
mod tests {
    use crate::{
        message::RequestPair, BlockBodies, BlockHeaders, GetBlockBodies, GetBlockHeaders,
        HeadersDirection,
    };
    use alloy_consensus::{Header, TxEip1559, TxLegacy};
    use alloy_eips::{eip2718::Encodable2718, eip4895::Withdrawal, BlockHashOrNumber};
    use alloy_primitives::{b256, hex, Address, Bloom, Signature, TxKind, B256, U256};
    use alloy_rlp::{Decodable, Encodable};
    use reth_ethereum_primitives::{BlockBody, Transaction, TransactionSigned};
    use std::str::FromStr;

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
        assert!(
            decode_result.is_err(),
            "Decoding a number longer than 64 bits (but not exactly 32 bytes) should not decode successfully"
        );
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
                start_block: BlockHashOrNumber::Hash(b256!(
                    "00000000000000000000000000000000000000000000000000000000deadc0de"
                )),
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
                start_block: BlockHashOrNumber::Hash(b256!(
                    "00000000000000000000000000000000000000000000000000000000deadc0de"
                )),
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
        RequestPair {
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
        let expected = RequestPair {
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
        let expected = hex!(
            "f90202820457f901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000"
        );
        let mut data = vec![];
        RequestPair {
            request_id: 1111,
            message: BlockHeaders(vec![Header {
                parent_hash: B256::ZERO,
                ommers_hash: B256::ZERO,
                beneficiary: Address::ZERO,
                state_root: B256::ZERO,
                transactions_root: B256::ZERO,
                receipts_root: B256::ZERO,
                logs_bloom: Bloom::ZERO,
                difficulty: U256::from(0x8aeu64),
                number: 0xd05u64,
                gas_limit: 0x115c,
                gas_used: 0x15b3,
                timestamp: 0x1a0au64,
                extra_data: hex!("7788")[..].into(),
                mix_hash: B256::ZERO,
                nonce: 0x0000000000000000u64.into(),
                base_fee_per_gas: None,
                withdrawals_root: None,
                blob_gas_used: None,
                excess_blob_gas: None,
                parent_beacon_block_root: None,
                requests_hash: None,
                block_access_list_hash: None,
                slot_number: None,
            }]),
        }
        .encode(&mut data);
        assert_eq!(data, expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn decode_block_header() {
        let data = hex!(
            "f90202820457f901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000"
        );
        let expected = RequestPair {
            request_id: 1111,
            message: BlockHeaders(vec![Header {
                parent_hash: B256::ZERO,
                ommers_hash: B256::ZERO,
                beneficiary: Address::ZERO,
                state_root: B256::ZERO,
                transactions_root: B256::ZERO,
                receipts_root: B256::ZERO,
                logs_bloom: Bloom::ZERO,
                difficulty: U256::from(0x8aeu64),
                number: 0xd05u64,
                gas_limit: 0x115c,
                gas_used: 0x15b3,
                timestamp: 0x1a0au64,
                extra_data: hex!("7788")[..].into(),
                mix_hash: B256::ZERO,
                nonce: 0x0000000000000000u64.into(),
                base_fee_per_gas: None,
                withdrawals_root: None,
                blob_gas_used: None,
                excess_blob_gas: None,
                parent_beacon_block_root: None,
                requests_hash: None,
                block_access_list_hash: None,
                slot_number: None,
            }]),
        };
        let result = RequestPair::decode(&mut &data[..]);
        assert_eq!(result.unwrap(), expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn encode_get_block_bodies() {
        let expected = hex!(
            "f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef"
        );
        let mut data = vec![];
        RequestPair {
            request_id: 1111,
            message: GetBlockBodies(vec![
                b256!("00000000000000000000000000000000000000000000000000000000deadc0de"),
                b256!("00000000000000000000000000000000000000000000000000000000feedbeef"),
            ]),
        }
        .encode(&mut data);
        assert_eq!(data, expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn decode_get_block_bodies() {
        let data = hex!(
            "f847820457f842a000000000000000000000000000000000000000000000000000000000deadc0dea000000000000000000000000000000000000000000000000000000000feedbeef"
        );
        let expected = RequestPair {
            request_id: 1111,
            message: GetBlockBodies(vec![
                b256!("00000000000000000000000000000000000000000000000000000000deadc0de"),
                b256!("00000000000000000000000000000000000000000000000000000000feedbeef"),
            ]),
        };
        let result = RequestPair::decode(&mut &data[..]);
        assert_eq!(result.unwrap(), expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn encode_block_bodies() {
        let expected = hex!(
            "f902dc820457f902d6f902d3f8d2f867088504a817c8088302e2489435353535353535353535353535353535353535358202008025a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10f867098504a817c809830334509435353535353535353535353535353535353535358202d98025a052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afba052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afbf901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000"
        );
        let mut data = vec![];
        let request = RequestPair {
            request_id: 1111,
            message: BlockBodies(vec![
                BlockBody {
                    transactions: vec![
                        TransactionSigned::new_unhashed(Transaction::Legacy(TxLegacy {
                            chain_id: Some(1),
                            nonce: 0x8u64,
                            gas_price: 0x4a817c808,
                            gas_limit: 0x2e248,
                            to: TxKind::Call(Address::repeat_byte(0x35)),
                            value: U256::from(0x200u64),
                            input: Default::default(),
                        }), Signature::new(
                                U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12").unwrap(),
                                U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10").unwrap(),
                                false,
                            ),
                        ),
                        TransactionSigned::new_unhashed(Transaction::Legacy(TxLegacy {
                            chain_id: Some(1),
                            nonce: 0x9u64,
                            gas_price: 0x4a817c809,
                            gas_limit: 0x33450,
                            to: TxKind::Call(Address::repeat_byte(0x35)),
                            value: U256::from(0x2d9u64),
                            input: Default::default(),
                        }), Signature::new(
                                U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                false,
                            ),
                        ),
                    ],
                    ommers: vec![
                        Header {
                            parent_hash: B256::ZERO,
                            ommers_hash: B256::ZERO,
                            beneficiary: Address::ZERO,
                            state_root: B256::ZERO,
                            transactions_root: B256::ZERO,
                            receipts_root: B256::ZERO,
                            logs_bloom: Bloom::ZERO,
                            difficulty: U256::from(0x8aeu64),
                            number: 0xd05u64,
                            gas_limit: 0x115c,
                            gas_used: 0x15b3,
                            timestamp: 0x1a0au64,
                            extra_data: hex!("7788")[..].into(),
                            mix_hash: B256::ZERO,
                            nonce: 0x0000000000000000u64.into(),
                            base_fee_per_gas: None,
                            withdrawals_root: None,
                            blob_gas_used: None,
                            excess_blob_gas: None,
                            parent_beacon_block_root: None,
                            requests_hash: None,
                            block_access_list_hash: None,
                            slot_number: None,
                        },
                    ],
                    withdrawals: None,
                }
            ]),
        };
        request.encode(&mut data);
        assert_eq!(data, expected);
    }

    // Test vector from: https://eips.ethereum.org/EIPS/eip-2481
    #[test]
    fn decode_block_bodies() {
        let data = hex!(
            "f902dc820457f902d6f902d3f8d2f867088504a817c8088302e2489435353535353535353535353535353535353535358202008025a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12a064b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10f867098504a817c809830334509435353535353535353535353535353535353535358202d98025a052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afba052f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afbf901fcf901f9a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000940000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000a00000000000000000000000000000000000000000000000000000000000000000b90100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000008208ae820d0582115c8215b3821a0a827788a00000000000000000000000000000000000000000000000000000000000000000880000000000000000"
        );
        let expected = RequestPair {
            request_id: 1111,
            message: BlockBodies(vec![
                BlockBody {
                    transactions: vec![
                        TransactionSigned::new_unhashed(Transaction::Legacy(
                            TxLegacy {
                                chain_id: Some(1),
                                nonce: 0x8u64,
                                gas_price: 0x4a817c808,
                                gas_limit: 0x2e248,
                                to: TxKind::Call(Address::repeat_byte(0x35)),
                                value: U256::from(0x200u64),
                                input: Default::default(),
                            }),
                                                        Signature::new(
                                U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c12").unwrap(),
                                U256::from_str("0x64b1702d9298fee62dfeccc57d322a463ad55ca201256d01f62b45b2e1c21c10").unwrap(),
                                false,
                            ),
                        ),
                        TransactionSigned::new_unhashed(
                            Transaction::Legacy(TxLegacy {
                                chain_id: Some(1),
                                nonce: 0x9u64,
                                gas_price: 0x4a817c809,
                                gas_limit: 0x33450,
                                to: TxKind::Call(Address::repeat_byte(0x35)),
                                value: U256::from(0x2d9u64),
                                input: Default::default(),
                            }),
                            Signature::new(
                                U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                U256::from_str("0x52f8f61201b2b11a78d6e866abc9c3db2ae8631fa656bfe5cb53668255367afb").unwrap(),
                                false,
                            ),
                        ),
                    ],
                    ommers: vec![
                        Header {
                            parent_hash: B256::ZERO,
                            ommers_hash: B256::ZERO,
                            beneficiary: Address::ZERO,
                            state_root: B256::ZERO,
                            transactions_root: B256::ZERO,
                            receipts_root: B256::ZERO,
                            logs_bloom: Bloom::ZERO,
                            difficulty: U256::from(0x8aeu64),
                            number: 0xd05u64,
                            gas_limit: 0x115c,
                            gas_used: 0x15b3,
                            timestamp: 0x1a0au64,
                            extra_data: hex!("7788")[..].into(),
                            mix_hash: B256::ZERO,
                            nonce: 0x0000000000000000u64.into(),
                            base_fee_per_gas: None,
                            withdrawals_root: None,
                            blob_gas_used: None,
                            excess_blob_gas: None,
                            parent_beacon_block_root: None,
                            requests_hash: None,
                            block_access_list_hash: None,
                            slot_number: None,
                        },
                    ],
                    withdrawals: None,
                }
            ]),
        };
        let result = RequestPair::decode(&mut &data[..]).unwrap();
        assert_eq!(result, expected);
    }

    #[test]
    fn empty_block_bodies_rlp() {
        let body = BlockBodies::default();
        let mut buf = Vec::new();
        body.encode(&mut buf);
        let decoded = BlockBodies::<BlockBody>::decode(&mut buf.as_slice()).unwrap();
        assert_eq!(body, decoded);
    }

    #[test]
    fn block_bodies_memory_budget() {
        let tx = TransactionSigned::new_unhashed(
            Transaction::Legacy(TxLegacy::default()),
            Signature::test_signature(),
        );
        let bodies = BlockBodies(vec![
            BlockBody {
                transactions: vec![tx.clone(); 5],
                ommers: vec![Header::default()],
                withdrawals: Some(vec![Withdrawal::default(); 3].into()),
            },
            BlockBody { transactions: vec![tx; 1], ..Default::default() },
        ]);
        let encoded = alloy_rlp::encode(&bodies);
        // Decoding grows the body list to 4 bodies, the transaction lists to 8 and 4, the ommer
        // list to 4 and the withdrawal list to 4.
        let size = 4 * core::mem::size_of::<BlockBody>() +
            12 * core::mem::size_of::<TransactionSigned>() +
            4 * core::mem::size_of::<Header>() +
            4 * core::mem::size_of::<Withdrawal>();

        let mut buf = encoded.as_slice();
        let decoded = BlockBodies::<BlockBody>::decode_with_memory_budget(&mut buf, size).unwrap();
        assert_eq!(decoded, bodies);
        assert!(buf.is_empty());

        assert_eq!(
            BlockBodies::<BlockBody>::decode_with_memory_budget(&mut encoded.as_slice(), size - 1),
            Err(alloy_rlp::Error::Custom("decoded list exceeds memory budget"))
        );
    }

    #[test]
    fn block_bodies_memory_budget_counts_transactions_as_decoded() {
        let list = |payload: &[u8]| {
            let mut out = Vec::new();
            alloy_rlp::Header { list: true, payload_length: payload.len() }.encode(&mut out);
            out.extend_from_slice(payload);
            out
        };
        let legacy = TransactionSigned::new_unhashed(
            Transaction::Legacy(TxLegacy::default()),
            Signature::test_signature(),
        );
        let typed = TransactionSigned::new_unhashed(
            Transaction::Eip1559(TxEip1559::default()),
            Signature::test_signature(),
        );

        // One RLP string holding a typed transaction followed by seven legacy transactions, which
        // the network decoder reads as eight transactions.
        let mut wrapped = typed.encoded_2718();
        for _ in 0..7 {
            legacy.encode(&mut wrapped);
        }
        let mut txs = Vec::new();
        alloy_rlp::Header { list: false, payload_length: wrapped.len() }.encode(&mut txs);
        txs.extend(wrapped);
        let encoded = list(&list(&[list(&txs), list(&[])].concat()));
        let size =
            4 * core::mem::size_of::<BlockBody>() + 8 * core::mem::size_of::<TransactionSigned>();

        let decoded =
            BlockBodies::<BlockBody>::decode_with_memory_budget(&mut encoded.as_slice(), size)
                .unwrap();
        assert_eq!(decoded.0[0].transactions.len(), 8);

        assert_eq!(
            BlockBodies::<BlockBody>::decode_with_memory_budget(&mut encoded.as_slice(), size - 1),
            Err(alloy_rlp::Error::Custom("decoded list exceeds memory budget"))
        );
    }
}
