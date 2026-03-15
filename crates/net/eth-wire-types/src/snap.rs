//! Implements Ethereum SNAP message types.
//! Snap protocol runs on top of `RLPx`
//! facilitating the exchange of Ethereum state snapshots between peers
//! Reference: [Ethereum Snapshot Protocol](https://github.com/ethereum/devp2p/blob/master/caps/snap.md#protocol-messages)
//!
//! Supported versions: snap/1, snap/2
//!
//! snap/2 replaces the trie healing messages (GetTrieNodes/TrieNodes at 0x06/0x07) with
//! BAL-based healing messages (GetBlockAccessLists/BlockAccessLists), enabling deterministic
//! state healing via sequential BAL application instead of iterative trie node fetching.
//! See: <https://ethresear.ch/t/snap-v2-replacing-trie-healing-with-bals/24333>

use alloc::{string::String, vec::Vec};
use alloy_primitives::{Bytes, B256};
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use core::fmt;
use reth_codecs_derive::add_arbitrary_tests;

/// The snap protocol version.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub enum SnapVersion {
    /// snap/1: original snap sync with trie healing via GetTrieNodes/TrieNodes.
    Snap1 = 1,
    /// snap/2: replaces trie healing with BAL-based healing via
    /// GetBlockAccessLists/BlockAccessLists.
    Snap2 = 2,
}

impl SnapVersion {
    /// The latest known snap version.
    pub const LATEST: Self = Self::Snap2;

    /// All supported snap versions, ordered newest first.
    pub const ALL_VERSIONS: &'static [Self] = &[Self::Snap2, Self::Snap1];

    /// Returns true if this is snap/1.
    pub const fn is_snap1(&self) -> bool {
        matches!(self, Self::Snap1)
    }

    /// Returns true if this is snap/2.
    pub const fn is_snap2(&self) -> bool {
        matches!(self, Self::Snap2)
    }
}

impl fmt::Display for SnapVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "snap/{}", *self as u8)
    }
}

impl TryFrom<u8> for SnapVersion {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Snap1),
            2 => Ok(Self::Snap2),
            _ => Err(alloc::format!("unknown snap version: {value}")),
        }
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
    GetTrieNodes = 0x06,
    /// Response for the number of requested state trie nodes.
    TrieNodes = 0x07,
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

/// Request for block access lists (snap/2, message ID 0x06).
///
/// In snap/2, this replaces `GetTrieNodes`. Nodes request BALs for blocks that advanced
/// during the state download phase, enabling deterministic healing.
///
/// Format: `[request-id: P, [blockhash₁: B_32, blockhash₂: B_32, ...]]`
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct GetBlockAccessListsSnapMessage {
    /// Request ID to match up responses with
    pub request_id: u64,
    /// Block hashes to retrieve block access lists for
    pub block_hashes: Vec<B256>,
}

/// Response containing block access lists (snap/2, message ID 0x07).
///
/// In snap/2, this replaces `TrieNodes`. Each BAL can be verified against its header
/// commitment: `keccak256(rlp(bal)) == header.block_access_list_hash`.
///
/// Format: `[request-id: P, [block-access-list₁, block-access-list₂, ...]]`
#[derive(Debug, Clone, PartialEq, Eq, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[add_arbitrary_tests(rlp)]
pub struct BlockAccessListsSnapMessage {
    /// ID of the request this is a response for
    pub request_id: u64,
    /// The requested block access lists, in the same order as the request.
    /// Empty entries (zero-length bytes) indicate unavailable BALs.
    pub access_lists: Vec<Bytes>,
}

/// Represents all types of messages in the snap sync protocol.
///
/// Messages 0x00–0x05 are shared across snap/1 and snap/2.
/// Messages 0x06–0x07 differ by version:
/// - snap/1: `GetTrieNodes` / `TrieNodes`
/// - snap/2: `GetBlockAccessLists` / `BlockAccessLists`
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
    /// Request for trie nodes (snap/1 only) - see [`GetTrieNodesMessage`]
    GetTrieNodes(GetTrieNodesMessage),
    /// Response with trie nodes (snap/1 only) - see [`TrieNodesMessage`]
    TrieNodes(TrieNodesMessage),
    /// Request for block access lists (snap/2 only) - see [`GetBlockAccessListsSnapMessage`]
    GetBlockAccessLists(GetBlockAccessListsSnapMessage),
    /// Response with block access lists (snap/2 only) - see [`BlockAccessListsSnapMessage`]
    BlockAccessLists(BlockAccessListsSnapMessage),
}

impl SnapProtocolMessage {
    /// Returns the protocol message ID for this message type.
    ///
    /// The message ID is used in the `RLPx` protocol to identify different types of messages.
    /// Note: snap/2's GetBlockAccessLists/BlockAccessLists reuse the same IDs (0x06/0x07)
    /// as snap/1's GetTrieNodes/TrieNodes.
    pub const fn message_id(&self) -> SnapMessageId {
        match self {
            Self::GetAccountRange(_) => SnapMessageId::GetAccountRange,
            Self::AccountRange(_) => SnapMessageId::AccountRange,
            Self::GetStorageRanges(_) => SnapMessageId::GetStorageRanges,
            Self::StorageRanges(_) => SnapMessageId::StorageRanges,
            Self::GetByteCodes(_) => SnapMessageId::GetByteCodes,
            Self::ByteCodes(_) => SnapMessageId::ByteCodes,
            Self::GetTrieNodes(_) | Self::GetBlockAccessLists(_) => SnapMessageId::GetTrieNodes,
            Self::TrieNodes(_) | Self::BlockAccessLists(_) => SnapMessageId::TrieNodes,
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
    ///
    /// For message IDs 0x06/0x07, the `snap_version` determines the decoded type:
    /// - snap/1: `GetTrieNodes` / `TrieNodes`
    /// - snap/2: `GetBlockAccessLists` / `BlockAccessLists`
    pub fn decode_versioned(
        snap_version: SnapVersion,
        message_id: u8,
        buf: &mut &[u8],
    ) -> Result<Self, alloy_rlp::Error> {
        match message_id {
            id if id == SnapMessageId::GetAccountRange as u8 => {
                Ok(Self::GetAccountRange(GetAccountRangeMessage::decode(buf)?))
            }
            id if id == SnapMessageId::AccountRange as u8 => {
                Ok(Self::AccountRange(AccountRangeMessage::decode(buf)?))
            }
            id if id == SnapMessageId::GetStorageRanges as u8 => {
                Ok(Self::GetStorageRanges(GetStorageRangesMessage::decode(buf)?))
            }
            id if id == SnapMessageId::StorageRanges as u8 => {
                Ok(Self::StorageRanges(StorageRangesMessage::decode(buf)?))
            }
            id if id == SnapMessageId::GetByteCodes as u8 => {
                Ok(Self::GetByteCodes(GetByteCodesMessage::decode(buf)?))
            }
            id if id == SnapMessageId::ByteCodes as u8 => {
                Ok(Self::ByteCodes(ByteCodesMessage::decode(buf)?))
            }
            id if id == SnapMessageId::GetTrieNodes as u8 => match snap_version {
                SnapVersion::Snap1 => Ok(Self::GetTrieNodes(GetTrieNodesMessage::decode(buf)?)),
                SnapVersion::Snap2 => {
                    Ok(Self::GetBlockAccessLists(GetBlockAccessListsSnapMessage::decode(buf)?))
                }
            },
            id if id == SnapMessageId::TrieNodes as u8 => match snap_version {
                SnapVersion::Snap1 => Ok(Self::TrieNodes(TrieNodesMessage::decode(buf)?)),
                SnapVersion::Snap2 => {
                    Ok(Self::BlockAccessLists(BlockAccessListsSnapMessage::decode(buf)?))
                }
            },
            _ => Err(alloy_rlp::Error::Custom("Unknown message ID")),
        }
    }

    /// Decodes a SNAP protocol message assuming snap/1.
    ///
    /// For backwards compatibility with code that doesn't track snap versions.
    pub fn decode(message_id: u8, buf: &mut &[u8]) -> Result<Self, alloy_rlp::Error> {
        Self::decode_versioned(SnapVersion::Snap1, message_id, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to create a B256 from a u64 for testing
    fn b256_from_u64(value: u64) -> B256 {
        B256::left_padding_from(&value.to_be_bytes())
    }

    // Helper function to test roundtrip encoding/decoding for a specific snap version
    fn test_roundtrip_versioned(snap_version: SnapVersion, original: SnapProtocolMessage) {
        let encoded = original.encode();

        // Verify the first byte matches the expected message ID
        assert_eq!(encoded[0], original.message_id() as u8);

        let mut buf = &encoded[1..];
        let decoded =
            SnapProtocolMessage::decode_versioned(snap_version, encoded[0], &mut buf).unwrap();

        // Verify the match
        assert_eq!(decoded, original);
    }

    // Helper for snap/1 roundtrip
    fn test_roundtrip(original: SnapProtocolMessage) {
        test_roundtrip_versioned(SnapVersion::Snap1, original);
    }

    #[test]
    fn test_all_message_roundtrips() {
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
    }

    #[test]
    fn test_snap2_message_roundtrips() {
        test_roundtrip_versioned(
            SnapVersion::Snap2,
            SnapProtocolMessage::GetBlockAccessLists(GetBlockAccessListsSnapMessage {
                request_id: 42,
                block_hashes: vec![b256_from_u64(100), b256_from_u64(101), b256_from_u64(102)],
            }),
        );

        test_roundtrip_versioned(
            SnapVersion::Snap2,
            SnapProtocolMessage::BlockAccessLists(BlockAccessListsSnapMessage {
                request_id: 42,
                access_lists: vec![
                    Bytes::from(vec![1, 2, 3]),
                    Bytes::from(vec![4, 5, 6]),
                    Bytes::new(), // empty = unavailable
                ],
            }),
        );
    }

    #[test]
    fn test_snap2_shared_messages_roundtrip() {
        // Messages 0x00-0x05 should decode identically under both snap/1 and snap/2
        let msg = SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
            request_id: 1,
            root_hash: b256_from_u64(1),
            starting_hash: b256_from_u64(2),
            limit_hash: b256_from_u64(3),
            response_bytes: 512,
        });

        let encoded = msg.encode();

        let mut buf1 = &encoded[1..];
        let decoded_v1 =
            SnapProtocolMessage::decode_versioned(SnapVersion::Snap1, encoded[0], &mut buf1)
                .unwrap();

        let mut buf2 = &encoded[1..];
        let decoded_v2 =
            SnapProtocolMessage::decode_versioned(SnapVersion::Snap2, encoded[0], &mut buf2)
                .unwrap();

        assert_eq!(decoded_v1, decoded_v2);
    }

    #[test]
    fn test_snap2_message_id_0x06_is_get_block_access_lists() {
        let msg = GetBlockAccessListsSnapMessage { request_id: 1, block_hashes: vec![B256::ZERO] };
        let snap_msg = SnapProtocolMessage::GetBlockAccessLists(msg);
        assert_eq!(snap_msg.message_id() as u8, 0x06);
    }

    #[test]
    fn test_snap2_message_id_0x07_is_block_access_lists() {
        let msg = BlockAccessListsSnapMessage { request_id: 1, access_lists: vec![Bytes::new()] };
        let snap_msg = SnapProtocolMessage::BlockAccessLists(msg);
        assert_eq!(snap_msg.message_id() as u8, 0x07);
    }

    #[test]
    fn test_unknown_message_id() {
        let data = Bytes::from(vec![1, 2, 3, 4]);
        let mut buf = data.as_ref();

        let result = SnapProtocolMessage::decode(255, &mut buf);

        assert!(result.is_err());
        if let Err(e) = result {
            assert_eq!(e.to_string(), "Unknown message ID");
        }
    }

    #[test]
    fn test_snap_version_display() {
        assert_eq!(SnapVersion::Snap1.to_string(), "snap/1");
        assert_eq!(SnapVersion::Snap2.to_string(), "snap/2");
    }

    #[test]
    fn test_snap_version_try_from() {
        assert_eq!(SnapVersion::try_from(1u8), Ok(SnapVersion::Snap1));
        assert_eq!(SnapVersion::try_from(2u8), Ok(SnapVersion::Snap2));
        assert!(SnapVersion::try_from(0u8).is_err());
        assert!(SnapVersion::try_from(3u8).is_err());
    }
}
