//! Request and response messages of the snap/2 protocol.

use crate::{snap::AccountData, BlockAccessLists};
use alloc::vec::Vec;
use alloy_primitives::{Bytes, B256, U256};
use alloy_rlp::{BufMut, Decodable, Encodable, RlpDecodable, RlpEncodable};
use reth_codecs_derive::add_arbitrary_tests;

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
    /// Storage slot hash of the first to retrieve; unbounded (served as `B256::ZERO`) when the
    /// wire encoding is an empty byte string.
    pub starting_hash: RangeBound,
    /// Storage slot hash after which to stop serving; unbounded (served as
    /// `B256::repeat_byte(0xff)`) when the wire encoding is an empty byte string.
    pub limit_hash: RangeBound,
    /// Soft limit at which to stop returning data
    pub response_bytes: u64,
}

/// A `snap/2` storage-range bound (`origin`/`limit` on [`GetStorageRangesMessage`]).
///
/// Encoded as either an empty byte string (unbounded) or a 32-byte hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub struct RangeBound(Option<B256>);

impl RangeBound {
    /// Returns the bound's hash, or `default` if it was encoded as an empty byte string.
    pub const fn unwrap_or(self, default: B256) -> B256 {
        match self.0 {
            Some(hash) => hash,
            None => default,
        }
    }
}

impl From<B256> for RangeBound {
    fn from(hash: B256) -> Self {
        Self(Some(hash))
    }
}

impl Encodable for RangeBound {
    fn encode(&self, out: &mut dyn BufMut) {
        match self.0 {
            Some(hash) => hash.encode(out),
            None => Bytes::new().encode(out),
        }
    }

    fn length(&self) -> usize {
        match self.0 {
            Some(hash) => hash.length(),
            None => Bytes::new().length(),
        }
    }
}

impl Decodable for RangeBound {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let bytes = Bytes::decode(buf)?;
        match bytes.len() {
            0 => Ok(Self(None)),
            32 => Ok(Self(Some(B256::from_slice(&bytes)))),
            _ => Err(alloy_rlp::Error::UnexpectedLength),
        }
    }
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

impl StorageData {
    /// Encodes a slot value as the storage trie leaf commits to it.
    pub fn from_value(hash: B256, value: U256) -> Self {
        Self { hash, data: alloy_rlp::encode(value).into() }
    }

    /// Decodes the slot value.
    pub fn value(&self) -> alloy_rlp::Result<U256> {
        alloy_rlp::decode_exact(&self.data)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_data_rejects_trailing_bytes() {
        let mut slot = StorageData::from_value(B256::repeat_byte(4), U256::ONE);
        slot.data = [slot.data.as_ref(), &[0x00]].concat().into();

        assert!(slot.value().is_err());
    }
}
