//! Implementation of consensus layer messages[ClayerConsensusMessage]
use alloy_rlp::{Decodable, Encodable, RlpDecodable, RlpEncodable};
use reth_codecs::derive_arbitrary;
use reth_primitives::{
    bytes::{Buf, BufMut},
    hex, keccak256, public_key_to_address, Address, Block, Bloom, Bytes, PeerId, Withdrawal, B256,
    U256,
};
use secp256k1::PublicKey;

use super::signature::ClayerSignature;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Consensus layer message header
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClayerConsensusMessageHeader {
    /// consensus type
    pub message_type: u8,
    /// consensus message hash
    pub content_hash: B256,
    /// node peer id
    pub signer_id: PeerId,
}

/// Consensus layer message
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClayerConsensusMessage {
    /// header bytes
    pub header_bytes: Bytes,
    /// consensus signature
    pub header_signature: ClayerSignature,
    /// message body
    pub message_bytes: Bytes,
}

/// Represents all common information used in a PBFT message
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PbftMessageInfo {
    /// pbft message type
    pub ptype: u8,
    /// pbft view number
    pub view: u64,
    /// pbft sequence number
    pub seq_num: u64,
    /// node id
    pub signer_id: PeerId,
}

impl std::fmt::Display for PbftMessageInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "(type: {:?}, view: {:?}, seq_num: {:?}, signer_id: {:?})",
            PbftMessageType::from(self.ptype),
            self.view,
            self.seq_num,
            hex::encode(self.signer_id.clone()),
        )
    }
}

/// Consensus message
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PbftMessage {
    /// pbft info
    pub info: PbftMessageInfo,
    /// pbft block hash
    pub block_id: B256,
}

/// Consensus layer message[Prepare]
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PbftSignedVote {
    /// header bytes
    pub header_bytes: Bytes,
    /// consensus signature
    pub header_signature: ClayerSignature,
    /// message body
    pub message_bytes: Bytes,
}

/// Consensus message seal
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PbftSeal {
    /// pbft info
    pub info: PbftMessageInfo,
    /// pbft block hash
    pub block_id: B256,
    /// a list of Commit votes to prove the block commit (must contain at least 2f votes)
    pub commit_votes: Vec<PbftSignedVote>,
}

/// Consensus message new view
#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PbftNewView {
    /// pbft info
    pub info: PbftMessageInfo,
    /// a list of Commit votes to prove the block commit (must contain at least 2f votes)
    pub view_changes: Vec<PbftSignedVote>,
}

// Messages related to PBFT consensus
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd)]
pub enum PbftMessageType {
    Unset = 0x00,
    /// Basic message types for the multicast protocol
    PrePrepare = 0x01,
    Prepare = 0x02,
    Commit = 0x03,

    /// Auxiliary PBFT messages
    NewView = 0x04,
    ViewChange = 0x05,
    SealRequest = 0x06,
    Seal = 0x07,
    BlockNew = 0x08,

    AnnounceBlock = 0x09,
}

impl std::fmt::Display for PbftMessageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let txt = match self {
            PbftMessageType::Unset => "Unset",
            PbftMessageType::PrePrepare => "PrePrepare",
            PbftMessageType::Prepare => "Prepare",
            PbftMessageType::Commit => "Commit",
            PbftMessageType::NewView => "NewView",
            PbftMessageType::ViewChange => "ViewChange",
            PbftMessageType::SealRequest => "SealRequest",
            PbftMessageType::Seal => "Seal",
            PbftMessageType::BlockNew => "BlockNew",
            PbftMessageType::AnnounceBlock => "AnnounceBlock",
        };
        write!(f, "{}", txt)
    }
}

impl From<u8> for PbftMessageType {
    fn from(value: u8) -> Self {
        match value {
            0x01 => PbftMessageType::PrePrepare,
            0x02 => PbftMessageType::Prepare,
            0x03 => PbftMessageType::Commit,
            0x04 => PbftMessageType::NewView,
            0x05 => PbftMessageType::ViewChange,
            0x06 => PbftMessageType::SealRequest,
            0x07 => PbftMessageType::Seal,
            0x08 => PbftMessageType::BlockNew,
            0x09 => PbftMessageType::AnnounceBlock,
            _ => PbftMessageType::Unset,
        }
    }
}

///
#[derive_arbitrary(rlp)]
#[derive(Clone, PartialEq, Eq, RlpEncodable, RlpDecodable, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClayerBlock {
    /// info
    pub info: PbftMessageInfo,
    /// block
    pub block: ClayerExecutionPayload,
    /// seal
    pub seal_bytes: Bytes,
}

impl ClayerBlock {
    /// Create a new `ClayerBlock`
    pub fn new(info: PbftMessageInfo, block: ClayerExecutionPayload, seal_bytes: Bytes) -> Self {
        ClayerBlock { info, block, seal_bytes }
    }

    /// Get the block id, call hash_slow for performance
    pub fn block_id(&self) -> B256 {
        self.block.block_hash.clone()
    }

    /// Get the previous block
    pub fn previous_id(&self) -> B256 {
        self.block.parent_hash.clone()
    }

    /// Get the block number
    pub fn block_num(&self) -> u64 {
        self.block.block_number
    }
}

impl std::hash::Hash for ClayerBlock {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.block.hash(state);
        self.seal_bytes.hash(state);
        self.info.hash(state);
    }
}

impl std::fmt::Debug for ClayerBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ClayerBlock(block_num: {:?}, block_id: {:?}, previous_id: {:?}, signer_id: {:?}",
            self.block_num(),
            self.block_id(),
            self.previous_id(),
            self.info.signer_id,
        )
    }
}

impl std::fmt::Display for ClayerBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ClayerBlock(block_num: {:?}, block_id: {:?}, previous_id: {:?}, signer_id: {:?}",
            self.block_num(),
            self.block_id(),
            self.previous_id(),
            self.info.signer_id,
        )
    }
}

/// ======================================================================================================================

#[derive_arbitrary(rlp)]
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable, Default, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ClayerExecutionPayload {
    ///
    pub parent_hash: B256,
    ///
    pub fee_recipient: Address,
    ///
    pub state_root: B256,
    ///
    pub receipts_root: B256,
    ///
    pub logs_bloom: Bloom,
    ///
    pub prev_randao: B256,
    ///
    pub block_number: u64,
    ///
    pub gas_limit: u64,
    ///
    pub gas_used: u64,
    ///
    pub timestamp: u64,
    ///
    pub extra_data: Bytes,
    ///
    pub base_fee_per_gas: U256,
    ///
    pub block_hash: B256,
    ///
    pub transactions: Vec<Bytes>,
    ///
    pub withdrawals: Vec<Withdrawal>,
}
