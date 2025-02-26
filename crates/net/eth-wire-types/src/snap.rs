//! Implements Ethereum SNAP message types.
//! Snap protocol runs on top of `RLPx`
//! facilitating the exchange of Ethereum state snapshots between peers
//! Reference: [Ethereum Snapshot Protocol](https://github.com/ethereum/devp2p/blob/master/caps/snap.md#protocol-messages)
//!
//! Current version: snap/1

use alloc::vec::Vec;
use alloy_primitives::{Bytes, B256};

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountData {
    /// Hash of the account address (trie path)
    pub hash: B256,
    /// Account body in slim format
    pub body: Bytes,
}

/// Response containing a number of consecutive accounts and the Merkle proofs for the entire range.
// http://github.com/ethereum/devp2p/blob/master/caps/snap.md#accountrange-0x01
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteCodesMessage {
    /// ID of the request this is a response for
    pub request_id: u64,
    /// The requested bytecodes in order
    pub codes: Vec<Bytes>,
}

/// Path in the trie for an account and its storage
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriePath {
    /// Path in the account trie
    pub account_path: Bytes,
    /// Paths in the storage trie
    pub slot_paths: Vec<Bytes>,
}

/// Request a number of state (either account or storage) Merkle trie nodes by path
// https://github.com/ethereum/devp2p/blob/master/caps/snap.md#gettrienodes-0x06
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrieNodesMessage {
    /// ID of the request this is a response for
    pub request_id: u64,
    /// The requested trie nodes in order
    pub nodes: Vec<Bytes>,
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
    GetTrieNodes(GetTrieNodesMessage),
    /// Response with trie nodes - see [`TrieNodesMessage`]
    TrieNodes(TrieNodesMessage),
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
        }
    }
}
