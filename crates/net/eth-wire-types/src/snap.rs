//! Implements Ethereum SNAP message types.
//! Snap protocol runs on top of `RLPx`
//! facilitating the exchange of Ethereum state snapshots between peers
//! Reference: [Ethereum Snapshot Protocol](https://github.com/ethereum/devp2p/blob/master/caps/snap.md#protocol-messages)
//!
//! This module currently includes snap/1 plus preparatory snap/2 message definitions.

use crate::BlockAccessLists;
use alloc::vec::Vec;
use alloy_primitives::{Bytes, B256};
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use reth_codecs_derive::add_arbitrary_tests;

/// Supported SNAP protocol versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum SnapVersion {
    /// The original snapshot protocol.
    #[default]
    V1 = 1,
    /// BAL-based healing as proposed by EIP-8189.
    V2 = 2,
}

impl SnapVersion {
    /// Returns the number of messages supported by this version.
    pub const fn message_count(self) -> u8 {
        match self {
            Self::V1 => 8,
            Self::V2 => 10,
        }
    }

    /// Returns the highest supported message id for this version.
    pub const fn max_message_id(self) -> u8 {
        self.message_count() - 1
    }
}

/// Message IDs for the snap sync protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapMessageId {
    /// Requests of an unknown number of accounts from a given account trie.
    GetAccountRange = 0x00,
    /// Response with the number of consecutive accounts and the Merkle proofs for the entire
    /// range.
    AccountRange = 0x01,
    /// Requests for the storage slots of multiple accounts' storage tries.
    GetStorageRanges = 0x02,
    /// Response for the number of consecutive storage slots for the requested account.
    StorageRanges = 0x03,
    /// Request of the number of contract byte-codes by hash.
    GetByteCodes = 0x04,
    /// Response for the number of requested contract codes.
    ByteCodes = 0x05,
    /// Request of the number of state (either account or storage) Merkle trie nodes by path.
    ///
    /// Only valid for `snap/1`. Replaced by BAL-based healing in `snap/2`.
    GetTrieNodes = 0x06,
    /// Response for the number of requested state trie nodes.
    ///
    /// Only valid for `snap/1`. Replaced by BAL-based healing in `snap/2`.
    TrieNodes = 0x07,
    /// Request BALs for a list of block hashes.
    ///
    /// Only valid for `snap/2`.
    GetBlockAccessLists = 0x08,
    /// Response containing BALs for the requested block hashes.
    ///
    /// Only valid for `snap/2`.
    BlockAccessLists = 0x09,
}

/// Request for a range of accounts from the state trie.
// https://github.com/ethereum/devp2p/blob/master/caps/snap.md#getaccountrange-0x00
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetAccountRangeMessage {
    /// Request ID to match up responses with
    pub request_id: u64,
    /// Root hash of the account trie to serve
    pub root_hash: B256,
    /// Account hash of the first to retrieve
    pub starting_hash: B256,
    /// Account hash after which to stop serving data
    pub limit_hash: B256,
    /// Soft limit at which to stop returning data
    pub response_bytes: u64,
}

/// Account data in the response.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct AccountData {
    /// Hash of the account address (trie path)
    pub hash: B256,
    /// Account body in slim format
    pub body: Bytes,
}

/// Response containing a number of consecutive accounts and the Merkle proofs for the entire range.
// http://github.com/ethereum/devp2p/blob/master/caps/snap.md#accountrange-0x01
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct AccountRangeMessage {
    /// ID of the request this is a response for
    pub request_id: u64,
    /// List of consecutive accounts from the trie
    pub accounts: Vec<AccountData>,
    /// List of trie nodes proving the account range
    pub proof: Vec<Bytes>,
}

/// Request for the storage slots of multiple accounts' storage tries.
// https://github.com/ethereum/devp2p/blob/master/caps/snap.md#getstorageranges-0x02
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetStorageRangesMessage {
    /// Request ID to match up responses with
    pub request_id: u64,
    /// Root hash of the account trie to serve
    pub root_hash: B256,
    /// Account hashes of the storage tries to serve
    pub account_hashes: Vec<B256>,
    /// Storage slot hash of the first to retrieve
    pub starting_hash: B256,
    /// Storage slot hash after which to stop serving
    pub limit_hash: B256,
    /// Soft limit at which to stop returning data
    pub response_bytes: u64,
}

/// Storage slot data in the response.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct StorageData {
    /// Hash of the storage slot key (trie path)
    pub hash: B256,
    /// Data content of the slot
    pub data: Bytes,
}

/// Response containing a number of consecutive storage slots for the requested account
/// and optionally the merkle proofs for the last range (boundary proofs) if it only partially
/// covers the storage trie.
// https://github.com/ethereum/devp2p/blob/master/caps/snap.md#storageranges-0x03
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct StorageRangesMessage {
    /// ID of the request this is a response for
    pub request_id: u64,
    /// List of list of consecutive slots from the trie (one list per account)
    pub slots: Vec<Vec<StorageData>>,
    /// List of trie nodes proving the slot range (if partial)
    pub proof: Vec<Bytes>,
}

/// Request to get a number of requested contract codes.
// https://github.com/ethereum/devp2p/blob/master/caps/snap.md#getbytecodes-0x04
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetByteCodesMessage {
    /// Request ID to match up responses with
    pub request_id: u64,
    /// Code hashes to retrieve the code for
    pub hashes: Vec<B256>,
    /// Soft limit at which to stop returning data (in bytes)
    pub response_bytes: u64,
}

/// Response containing a number of requested contract codes.
// https://github.com/ethereum/devp2p/blob/master/caps/snap.md#bytecodes-0x05
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct ByteCodesMessage {
    /// ID of the request this is a response for
    pub request_id: u64,
    /// The requested bytecodes in order
    pub codes: Vec<Bytes>,
}

/// Path in the trie for an account and its storage
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct TriePath {
    /// Path in the account trie
    pub account_path: Bytes,
    /// Paths in the storage trie
    pub slot_paths: Vec<Bytes>,
}

/// Request a number of state (either account or storage) Merkle trie nodes by path
// https://github.com/ethereum/devp2p/blob/master/caps/snap.md#gettrienodes-0x06
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetTrieNodesMessage {
    /// Request ID to match up responses with
    pub request_id: u64,
    /// Root hash of the account trie to serve
    pub root_hash: B256,
    /// Trie paths to retrieve the nodes for, grouped by account
    pub paths: Vec<TriePath>,
    /// Soft limit at which to stop returning data (in bytes)
    pub response_bytes: u64,
}

/// Response containing a number of requested state trie nodes
// https://github.com/ethereum/devp2p/blob/master/caps/snap.md#trienodes-0x07
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct TrieNodesMessage {
    /// ID of the request this is a response for
    pub request_id: u64,
    /// The requested trie nodes in order
    pub nodes: Vec<Bytes>,
}

/// Request BALs for the given block hashes.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetBlockAccessListsMessage {
    /// Request ID to match up responses with.
    pub request_id: u64,
    /// Block hashes to retrieve BALs for.
    pub block_hashes: Vec<B256>,
    /// Soft limit at which to stop returning data (in bytes).
    pub response_bytes: u64,
}

/// Response containing one BAL per requested block hash.
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct BlockAccessListsMessage {
    /// ID of the request this is a response for.
    pub request_id: u64,
    /// Raw BAL payloads in request order.
    pub block_access_lists: BlockAccessLists,
}

/// Represents all types of messages in the snap sync protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapProtocolMessage {
    /// Request for an account range - see [`GetAccountRangeMessage`]
    GetAccountRange(GetAccountRangeMessage),
    /// Response with accounts and proofs - see [`AccountRangeMessage`]
    AccountRange(AccountRangeMessage),
    /// Request for storage slots - see [`GetStorageRangesMessage`]
    GetStorageRanges(GetStorageRangesMessage),
    /// Response with storage slots - see [`StorageRangesMessage`]
    StorageRanges(StorageRangesMessage),
    /// Request for contract bytecodes - see [`GetByteCodesMessage`]
    GetByteCodes(GetByteCodesMessage),
    /// Response with contract codes - see [`ByteCodesMessage`]
    ByteCodes(ByteCodesMessage),
    /// Request for trie nodes - see [`GetTrieNodesMessage`]
    ///
    /// Only valid for `snap/1`. Replaced by BAL-based healing in `snap/2`.
    GetTrieNodes(GetTrieNodesMessage),
    /// Response with trie nodes - see [`TrieNodesMessage`]
    ///
    /// Only valid for `snap/1`. Replaced by BAL-based healing in `snap/2`.
    TrieNodes(TrieNodesMessage),
    /// Request for block access lists - see [`GetBlockAccessListsMessage`]
    ///
    /// Only valid for `snap/2`.
    GetBlockAccessLists(GetBlockAccessListsMessage),
    /// Response with block access lists - see [`BlockAccessListsMessage`]
    ///
    /// Only valid for `snap/2`.
    BlockAccessLists(BlockAccessListsMessage),
}

impl SnapProtocolMessage {
    /// Returns the protocol message ID for this message type.
    ///
    /// The message ID is used in the `RLPx` protocol to identify different types of messages.
    pub const fn message_id(&self) -> SnapMessageId {
        match self {
            Self::GetAccountRange(_) => SnapMessageId::GetAccountRange,
            Self::AccountRange(_) => SnapMessageId::AccountRange,
            Self::GetStorageRanges(_) => SnapMessageId::GetStorageRanges,
            Self::StorageRanges(_) => SnapMessageId::StorageRanges,
            Self::GetByteCodes(_) => SnapMessageId::GetByteCodes,
            Self::ByteCodes(_) => SnapMessageId::ByteCodes,
            Self::GetTrieNodes(_) => SnapMessageId::GetTrieNodes,
            Self::TrieNodes(_) => SnapMessageId::TrieNodes,
            Self::GetBlockAccessLists(_) => SnapMessageId::GetBlockAccessLists,
            Self::BlockAccessLists(_) => SnapMessageId::BlockAccessLists,
        }
    }

    /// Encode the message to bytes
    pub fn encode(&self) -> Bytes {
        let mut buf = Vec::new();
        // Add message ID as first byte
        buf.push(self.message_id() as u8);

        // Encode the message body based on its type
        match self {
            Self::GetAccountRange(msg) => msg.encode(&mut buf),
            Self::AccountRange(msg) => msg.encode(&mut buf),
            Self::GetStorageRanges(msg) => msg.encode(&mut buf),
            Self::StorageRanges(msg) => msg.encode(&mut buf),
            Self::GetByteCodes(msg) => msg.encode(&mut buf),
            Self::ByteCodes(msg) => msg.encode(&mut buf),
            Self::GetTrieNodes(msg) => msg.encode(&mut buf),
            Self::TrieNodes(msg) => msg.encode(&mut buf),
            Self::GetBlockAccessLists(msg) => msg.encode(&mut buf),
            Self::BlockAccessLists(msg) => msg.encode(&mut buf),
        }

        Bytes::from(buf)
    }

    /// Decodes a SNAP protocol message from its message ID and RLP-encoded body.
    pub fn decode(message_id: u8, buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        // Decoding protocol message variants based on message ID
        macro_rules! decode_snap_message_variant {
            ($message_id:expr, $buf:expr, $id:expr, $variant:ident, $msg_type:ty) => {
                if $message_id == $id as u8 {
                    return Ok(Self::$variant(<$msg_type>::decode($buf)?));
                }
            };
        }

        // Try to decode each message type based on the message ID
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::GetAccountRange,
            GetAccountRange,
            GetAccountRangeMessage
        );
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::AccountRange,
            AccountRange,
            AccountRangeMessage
        );
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::GetStorageRanges,
            GetStorageRanges,
            GetStorageRangesMessage
        );
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::StorageRanges,
            StorageRanges,
            StorageRangesMessage
        );
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::GetByteCodes,
            GetByteCodes,
            GetByteCodesMessage
        );
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::ByteCodes,
            ByteCodes,
            ByteCodesMessage
        );
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::GetTrieNodes,
            GetTrieNodes,
            GetTrieNodesMessage
        );
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::TrieNodes,
            TrieNodes,
            TrieNodesMessage
        );
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::GetBlockAccessLists,
            GetBlockAccessLists,
            GetBlockAccessListsMessage
        );
        decode_snap_message_variant!(
            message_id,
            buf,
            SnapMessageId::BlockAccessLists,
            BlockAccessLists,
            BlockAccessListsMessage
        );

        Err(alloy_rlp::Error::Custom("Unknown message ID"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create a B256 from a u64 for testing
    fn b256_from_u64(value: u64) -> B256 {
        B256::left_padding_from(&value.to_be_bytes())
    }

    // Helper function to test roundtrip encoding/decoding
    fn test_roundtrip(original: SnapProtocolMessage) {
        let encoded = original.encode();

        // Verify the first byte matches the expected message ID
        assert_eq!(encoded[0], original.message_id() as u8);

        let mut buf = &encoded[1..];
        let decoded = SnapProtocolMessage::decode(encoded[0], &mut buf).unwrap();

        // Verify the match
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_all_message_roundtrips() {
        assert_eq!(SnapVersion::V1.message_count(), 8);
        assert_eq!(SnapVersion::V2.message_count(), 10);

        test_roundtrip(SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
            request_id: 42,
            root_hash: b256_from_u64(123),
            starting_hash: b256_from_u64(456),
            limit_hash: b256_from_u64(789),
            response_bytes: 1024,
        }));

        test_roundtrip(SnapProtocolMessage::AccountRange(AccountRangeMessage {
            request_id: 42,
            accounts: vec![AccountData {
                hash: b256_from_u64(123),
                body: Bytes::from(vec![1, 2, 3]),
            }],
            proof: vec![Bytes::from(vec![4, 5, 6])],
        }));

        test_roundtrip(SnapProtocolMessage::GetStorageRanges(GetStorageRangesMessage {
            request_id: 42,
            root_hash: b256_from_u64(123),
            account_hashes: vec![b256_from_u64(456)],
            starting_hash: b256_from_u64(789),
            limit_hash: b256_from_u64(101112),
            response_bytes: 2048,
        }));

        test_roundtrip(SnapProtocolMessage::StorageRanges(StorageRangesMessage {
            request_id: 42,
            slots: vec![vec![StorageData {
                hash: b256_from_u64(123),
                data: Bytes::from(vec![1, 2, 3]),
            }]],
            proof: vec![Bytes::from(vec![4, 5, 6])],
        }));

        test_roundtrip(SnapProtocolMessage::GetByteCodes(GetByteCodesMessage {
            request_id: 42,
            hashes: vec![b256_from_u64(123)],
            response_bytes: 1024,
        }));

        test_roundtrip(SnapProtocolMessage::ByteCodes(ByteCodesMessage {
            request_id: 42,
            codes: vec![Bytes::from(vec![1, 2, 3])],
        }));

        test_roundtrip(SnapProtocolMessage::GetTrieNodes(GetTrieNodesMessage {
            request_id: 42,
            root_hash: b256_from_u64(123),
            paths: vec![TriePath {
                account_path: Bytes::from(vec![1, 2, 3]),
                slot_paths: vec![Bytes::from(vec![4, 5, 6])],
            }],
            response_bytes: 1024,
        }));

        test_roundtrip(SnapProtocolMessage::TrieNodes(TrieNodesMessage {
            request_id: 42,
            nodes: vec![Bytes::from(vec![1, 2, 3])],
        }));

        test_roundtrip(SnapProtocolMessage::GetBlockAccessLists(GetBlockAccessListsMessage {
            request_id: 42,
            block_hashes: vec![b256_from_u64(123), b256_from_u64(456)],
            response_bytes: 4096,
        }));

        test_roundtrip(SnapProtocolMessage::BlockAccessLists(BlockAccessListsMessage {
            request_id: 42,
            block_access_lists: BlockAccessLists(vec![
                Bytes::from_static(&[alloy_rlp::EMPTY_LIST_CODE]),
                Bytes::from_static(&[0xc1, alloy_rlp::EMPTY_LIST_CODE]),
            ]),
        }));
    }

    #[test]
    fn test_unknown_message_id() {
        // Create some random data
        let data = Bytes::from(vec![1, 2, 3, 4]);
        let mut buf = data.as_ref();

        // Try to decode with an invalid message ID
        let result = SnapProtocolMessage::decode(255, &mut buf);

        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.to_string(), "Unknown message ID");
        }
    }
}
