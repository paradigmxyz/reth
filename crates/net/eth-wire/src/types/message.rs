#![allow(missing_docs)]

use super::{
    broadcast::NewBlockHashes, BlockBodies, BlockHeaders, GetBlockBodies, GetBlockHeaders,
    GetNodeData, GetPooledTransactions, GetReceipts, NewBlock, NewPooledTransactionHashes66,
    NewPooledTransactionHashes68, NodeData, PooledTransactions, Receipts, Status, Transactions,
};
use crate::{errors::EthStreamError, EthVersion, SharedTransactions};
use alloy_rlp::{length_of_length, Decodable, Encodable, Header};
use reth_primitives::bytes::{Buf, BufMut};
use std::{fmt::Debug, sync::Arc};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// [`MAX_MESSAGE_SIZE`] is the maximum cap on the size of a protocol message.
// https://github.com/ethereum/go-ethereum/blob/30602163d5d8321fbc68afdcbbaf2362b2641bde/eth/protocols/eth/protocol.go#L50
pub const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// An `eth` protocol message, containing a message ID and payload.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ProtocolMessage {
    pub message_type: EthMessageID,
    pub message: EthMessage,
}

impl ProtocolMessage {
    /// Create a new ProtocolMessage from a message type and message rlp bytes.
    pub fn decode_message(version: EthVersion, buf: &mut &[u8]) -> Result<Self, EthStreamError> {
        let message_type = EthMessageID::decode(buf)?;

        let message = match message_type {
            EthMessageID::Status => EthMessage::Status(Status::decode(buf)?),
            EthMessageID::NewBlockHashes => {
                EthMessage::NewBlockHashes(NewBlockHashes::decode(buf)?)
            }
            EthMessageID::NewBlock => EthMessage::NewBlock(Box::new(NewBlock::decode(buf)?)),
            EthMessageID::Transactions => EthMessage::Transactions(Transactions::decode(buf)?),
            EthMessageID::NewPooledTransactionHashes => {
                if version >= EthVersion::Eth68 {
                    EthMessage::NewPooledTransactionHashes68(NewPooledTransactionHashes68::decode(
                        buf,
                    )?)
                } else {
                    EthMessage::NewPooledTransactionHashes66(NewPooledTransactionHashes66::decode(
                        buf,
                    )?)
                }
            }
            EthMessageID::GetBlockHeaders => {
                let request_pair = RequestPair::<GetBlockHeaders>::decode(buf)?;
                EthMessage::GetBlockHeaders(request_pair)
            }
            EthMessageID::BlockHeaders => {
                let request_pair = RequestPair::<BlockHeaders>::decode(buf)?;
                EthMessage::BlockHeaders(request_pair)
            }
            EthMessageID::GetBlockBodies => {
                let request_pair = RequestPair::<GetBlockBodies>::decode(buf)?;
                EthMessage::GetBlockBodies(request_pair)
            }
            EthMessageID::BlockBodies => {
                let request_pair = RequestPair::<BlockBodies>::decode(buf)?;
                EthMessage::BlockBodies(request_pair)
            }
            EthMessageID::GetPooledTransactions => {
                let request_pair = RequestPair::<GetPooledTransactions>::decode(buf)?;
                EthMessage::GetPooledTransactions(request_pair)
            }
            EthMessageID::PooledTransactions => {
                let request_pair = RequestPair::<PooledTransactions>::decode(buf)?;
                EthMessage::PooledTransactions(request_pair)
            }
            EthMessageID::GetNodeData => {
                if version >= EthVersion::Eth67 {
                    return Err(EthStreamError::EthInvalidMessageError(
                        version,
                        EthMessageID::GetNodeData,
                    ))
                }
                let request_pair = RequestPair::<GetNodeData>::decode(buf)?;
                EthMessage::GetNodeData(request_pair)
            }
            EthMessageID::NodeData => {
                if version >= EthVersion::Eth67 {
                    return Err(EthStreamError::EthInvalidMessageError(
                        version,
                        EthMessageID::GetNodeData,
                    ))
                }
                let request_pair = RequestPair::<NodeData>::decode(buf)?;
                EthMessage::NodeData(request_pair)
            }
            EthMessageID::GetReceipts => {
                let request_pair = RequestPair::<GetReceipts>::decode(buf)?;
                EthMessage::GetReceipts(request_pair)
            }
            EthMessageID::Receipts => {
                let request_pair = RequestPair::<Receipts>::decode(buf)?;
                EthMessage::Receipts(request_pair)
            }
        };
        Ok(ProtocolMessage { message_type, message })
    }
}

impl Encodable for ProtocolMessage {
    /// Encodes the protocol message into bytes. The message type is encoded as a single byte and
    /// prepended to the message.
    fn encode(&self, out: &mut dyn BufMut) {
        self.message_type.encode(out);
        self.message.encode(out);
    }
    fn length(&self) -> usize {
        self.message_type.length() + self.message.length()
    }
}

impl From<EthMessage> for ProtocolMessage {
    fn from(message: EthMessage) -> Self {
        ProtocolMessage { message_type: message.message_id(), message }
    }
}

/// Represents messages that can be sent to multiple peers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolBroadcastMessage {
    pub message_type: EthMessageID,
    pub message: EthBroadcastMessage,
}

impl Encodable for ProtocolBroadcastMessage {
    /// Encodes the protocol message into bytes. The message type is encoded as a single byte and
    /// prepended to the message.
    fn encode(&self, out: &mut dyn BufMut) {
        self.message_type.encode(out);
        self.message.encode(out);
    }
    fn length(&self) -> usize {
        self.message_type.length() + self.message.length()
    }
}

impl From<EthBroadcastMessage> for ProtocolBroadcastMessage {
    fn from(message: EthBroadcastMessage) -> Self {
        ProtocolBroadcastMessage { message_type: message.message_id(), message }
    }
}

/// Represents a message in the eth wire protocol, versions 66, 67 and 68.
///
/// The ethereum wire protocol is a set of messages that are broadcast to the network in two
/// styles:
///  * A request message sent by a peer (such as [`GetPooledTransactions`]), and an associated
///  response message (such as [`PooledTransactions`]).
///  * A message that is broadcast to the network, without a corresponding request.
///
/// The newer `eth/66` is an efficiency upgrade on top of `eth/65`, introducing a request id to
/// correlate request-response message pairs. This allows for request multiplexing.
///
/// The `eth/67` is based on `eth/66` but only removes two messages, [`GetNodeData`] and
/// [``NodeData].
///
/// The `eth/68` changes only NewPooledTransactionHashes to include `types` and `sized`. For
/// it, NewPooledTransactionHashes is renamed as [`NewPooledTransactionHashes66`] and
/// [`NewPooledTransactionHashes68`] is defined.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum EthMessage {
    /// Status is required for the protocol handshake
    Status(Status),
    /// The following messages are broadcast to the network
    NewBlockHashes(NewBlockHashes),
    NewBlock(Box<NewBlock>),
    Transactions(Transactions),
    NewPooledTransactionHashes66(NewPooledTransactionHashes66),
    NewPooledTransactionHashes68(NewPooledTransactionHashes68),

    // The following messages are request-response message pairs
    GetBlockHeaders(RequestPair<GetBlockHeaders>),
    BlockHeaders(RequestPair<BlockHeaders>),
    GetBlockBodies(RequestPair<GetBlockBodies>),
    BlockBodies(RequestPair<BlockBodies>),
    GetPooledTransactions(RequestPair<GetPooledTransactions>),
    PooledTransactions(RequestPair<PooledTransactions>),
    GetNodeData(RequestPair<GetNodeData>),
    NodeData(RequestPair<NodeData>),
    GetReceipts(RequestPair<GetReceipts>),
    Receipts(RequestPair<Receipts>),
}

impl EthMessage {
    /// Returns the message's ID.
    pub fn message_id(&self) -> EthMessageID {
        match self {
            EthMessage::Status(_) => EthMessageID::Status,
            EthMessage::NewBlockHashes(_) => EthMessageID::NewBlockHashes,
            EthMessage::NewBlock(_) => EthMessageID::NewBlock,
            EthMessage::Transactions(_) => EthMessageID::Transactions,
            EthMessage::NewPooledTransactionHashes66(_) |
            EthMessage::NewPooledTransactionHashes68(_) => EthMessageID::NewPooledTransactionHashes,
            EthMessage::GetBlockHeaders(_) => EthMessageID::GetBlockHeaders,
            EthMessage::BlockHeaders(_) => EthMessageID::BlockHeaders,
            EthMessage::GetBlockBodies(_) => EthMessageID::GetBlockBodies,
            EthMessage::BlockBodies(_) => EthMessageID::BlockBodies,
            EthMessage::GetPooledTransactions(_) => EthMessageID::GetPooledTransactions,
            EthMessage::PooledTransactions(_) => EthMessageID::PooledTransactions,
            EthMessage::GetNodeData(_) => EthMessageID::GetNodeData,
            EthMessage::NodeData(_) => EthMessageID::NodeData,
            EthMessage::GetReceipts(_) => EthMessageID::GetReceipts,
            EthMessage::Receipts(_) => EthMessageID::Receipts,
        }
    }
}

impl Encodable for EthMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            EthMessage::Status(status) => status.encode(out),
            EthMessage::NewBlockHashes(new_block_hashes) => new_block_hashes.encode(out),
            EthMessage::NewBlock(new_block) => new_block.encode(out),
            EthMessage::Transactions(transactions) => transactions.encode(out),
            EthMessage::NewPooledTransactionHashes66(hashes) => hashes.encode(out),
            EthMessage::NewPooledTransactionHashes68(hashes) => hashes.encode(out),
            EthMessage::GetBlockHeaders(request) => request.encode(out),
            EthMessage::BlockHeaders(headers) => headers.encode(out),
            EthMessage::GetBlockBodies(request) => request.encode(out),
            EthMessage::BlockBodies(bodies) => bodies.encode(out),
            EthMessage::GetPooledTransactions(request) => request.encode(out),
            EthMessage::PooledTransactions(transactions) => transactions.encode(out),
            EthMessage::GetNodeData(request) => request.encode(out),
            EthMessage::NodeData(data) => data.encode(out),
            EthMessage::GetReceipts(request) => request.encode(out),
            EthMessage::Receipts(receipts) => receipts.encode(out),
        }
    }
    fn length(&self) -> usize {
        match self {
            EthMessage::Status(status) => status.length(),
            EthMessage::NewBlockHashes(new_block_hashes) => new_block_hashes.length(),
            EthMessage::NewBlock(new_block) => new_block.length(),
            EthMessage::Transactions(transactions) => transactions.length(),
            EthMessage::NewPooledTransactionHashes66(hashes) => hashes.length(),
            EthMessage::NewPooledTransactionHashes68(hashes) => hashes.length(),
            EthMessage::GetBlockHeaders(request) => request.length(),
            EthMessage::BlockHeaders(headers) => headers.length(),
            EthMessage::GetBlockBodies(request) => request.length(),
            EthMessage::BlockBodies(bodies) => bodies.length(),
            EthMessage::GetPooledTransactions(request) => request.length(),
            EthMessage::PooledTransactions(transactions) => transactions.length(),
            EthMessage::GetNodeData(request) => request.length(),
            EthMessage::NodeData(data) => data.length(),
            EthMessage::GetReceipts(request) => request.length(),
            EthMessage::Receipts(receipts) => receipts.length(),
        }
    }
}

/// Represents broadcast messages of [`EthMessage`] with the same object that can be sent to
/// multiple peers.
///
/// Messages that contain a list of hashes depend on the peer the message is sent to. A peer should
/// never receive a hash of an object (block, transaction) it has already seen.
///
/// Note: This is only useful for outgoing messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EthBroadcastMessage {
    NewBlock(Arc<NewBlock>),
    Transactions(SharedTransactions),
}

// === impl EthBroadcastMessage ===

impl EthBroadcastMessage {
    /// Returns the message's ID.
    pub fn message_id(&self) -> EthMessageID {
        match self {
            EthBroadcastMessage::NewBlock(_) => EthMessageID::NewBlock,
            EthBroadcastMessage::Transactions(_) => EthMessageID::Transactions,
        }
    }
}

impl Encodable for EthBroadcastMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            EthBroadcastMessage::NewBlock(new_block) => new_block.encode(out),
            EthBroadcastMessage::Transactions(transactions) => transactions.encode(out),
        }
    }

    fn length(&self) -> usize {
        match self {
            EthBroadcastMessage::NewBlock(new_block) => new_block.length(),
            EthBroadcastMessage::Transactions(transactions) => transactions.length(),
        }
    }
}

/// Represents message IDs for eth protocol messages.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum EthMessageID {
    Status = 0x00,
    NewBlockHashes = 0x01,
    Transactions = 0x02,
    GetBlockHeaders = 0x03,
    BlockHeaders = 0x04,
    GetBlockBodies = 0x05,
    BlockBodies = 0x06,
    NewBlock = 0x07,
    NewPooledTransactionHashes = 0x08,
    GetPooledTransactions = 0x09,
    PooledTransactions = 0x0a,
    GetNodeData = 0x0d,
    NodeData = 0x0e,
    GetReceipts = 0x0f,
    Receipts = 0x10,
}

impl EthMessageID {
    /// Returns the max value.
    pub const fn max() -> u8 {
        Self::Receipts as u8
    }
}

impl Encodable for EthMessageID {
    fn encode(&self, out: &mut dyn BufMut) {
        out.put_u8(*self as u8);
    }
    fn length(&self) -> usize {
        1
    }
}

impl Decodable for EthMessageID {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let id = buf.first().ok_or(alloy_rlp::Error::InputTooShort)?;
        let id = match id {
            0x00 => EthMessageID::Status,
            0x01 => EthMessageID::NewBlockHashes,
            0x02 => EthMessageID::Transactions,
            0x03 => EthMessageID::GetBlockHeaders,
            0x04 => EthMessageID::BlockHeaders,
            0x05 => EthMessageID::GetBlockBodies,
            0x06 => EthMessageID::BlockBodies,
            0x07 => EthMessageID::NewBlock,
            0x08 => EthMessageID::NewPooledTransactionHashes,
            0x09 => EthMessageID::GetPooledTransactions,
            0x0a => EthMessageID::PooledTransactions,
            0x0d => EthMessageID::GetNodeData,
            0x0e => EthMessageID::NodeData,
            0x0f => EthMessageID::GetReceipts,
            0x10 => EthMessageID::Receipts,
            _ => return Err(alloy_rlp::Error::Custom("Invalid message ID")),
        };
        buf.advance(1);
        Ok(id)
    }
}

impl TryFrom<usize> for EthMessageID {
    type Error = &'static str;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(EthMessageID::Status),
            0x01 => Ok(EthMessageID::NewBlockHashes),
            0x02 => Ok(EthMessageID::Transactions),
            0x03 => Ok(EthMessageID::GetBlockHeaders),
            0x04 => Ok(EthMessageID::BlockHeaders),
            0x05 => Ok(EthMessageID::GetBlockBodies),
            0x06 => Ok(EthMessageID::BlockBodies),
            0x07 => Ok(EthMessageID::NewBlock),
            0x08 => Ok(EthMessageID::NewPooledTransactionHashes),
            0x09 => Ok(EthMessageID::GetPooledTransactions),
            0x0a => Ok(EthMessageID::PooledTransactions),
            0x0d => Ok(EthMessageID::GetNodeData),
            0x0e => Ok(EthMessageID::NodeData),
            0x0f => Ok(EthMessageID::GetReceipts),
            0x10 => Ok(EthMessageID::Receipts),
            _ => Err("Invalid message ID"),
        }
    }
}

/// This is used for all request-response style `eth` protocol messages.
/// This can represent either a request or a response, since both include a message payload and
/// request id.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RequestPair<T> {
    /// id for the contained request or response message
    pub request_id: u64,

    /// the request or response message payload
    pub message: T,
}

/// Allows messages with request ids to be serialized into RLP bytes.
impl<T> Encodable for RequestPair<T>
where
    T: Encodable,
{
    fn encode(&self, out: &mut dyn alloy_rlp::BufMut) {
        let header =
            Header { list: true, payload_length: self.request_id.length() + self.message.length() };

        header.encode(out);
        self.request_id.encode(out);
        self.message.encode(out);
    }

    fn length(&self) -> usize {
        let mut length = 0;
        length += self.request_id.length();
        length += self.message.length();
        length += length_of_length(length);
        length
    }
}

/// Allows messages with request ids to be deserialized into RLP bytes.
impl<T> Decodable for RequestPair<T>
where
    T: Decodable,
{
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let _header = Header::decode(buf)?;
        Ok(Self { request_id: u64::decode(buf)?, message: T::decode(buf)? })
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        errors::EthStreamError, types::message::RequestPair, EthMessage, EthMessageID, GetNodeData,
        NodeData, ProtocolMessage,
    };
    use alloy_rlp::{Decodable, Encodable};
    use reth_primitives::hex;

    fn encode<T: Encodable>(value: T) -> Vec<u8> {
        let mut buf = vec![];
        value.encode(&mut buf);
        buf
    }

    #[test]
    fn test_removed_message_at_eth67() {
        let get_node_data =
            EthMessage::GetNodeData(RequestPair { request_id: 1337, message: GetNodeData(vec![]) });
        let buf = encode(ProtocolMessage {
            message_type: EthMessageID::GetNodeData,
            message: get_node_data,
        });
        let msg = ProtocolMessage::decode_message(crate::EthVersion::Eth67, &mut &buf[..]);
        assert!(matches!(msg, Err(EthStreamError::EthInvalidMessageError(..))));

        let node_data =
            EthMessage::NodeData(RequestPair { request_id: 1337, message: NodeData(vec![]) });
        let buf =
            encode(ProtocolMessage { message_type: EthMessageID::NodeData, message: node_data });
        let msg = ProtocolMessage::decode_message(crate::EthVersion::Eth67, &mut &buf[..]);
        assert!(matches!(msg, Err(EthStreamError::EthInvalidMessageError(..))));
    }

    #[test]
    fn request_pair_encode() {
        let request_pair = RequestPair { request_id: 1337, message: vec![5u8] };

        // c5: start of list (c0) + len(full_list) (length is <55 bytes)
        // 82: 0x80 + len(1337)
        // 05 39: 1337 (request_id)
        // === full_list ===
        // c1: start of list (c0) + len(list) (length is <55 bytes)
        // 05: 5 (message)
        let expected = hex!("c5820539c105");
        let got = encode(request_pair);
        assert_eq!(expected[..], got, "expected: {expected:X?}, got: {got:X?}",);
    }

    #[test]
    fn request_pair_decode() {
        let raw_pair = &hex!("c5820539c105")[..];

        let expected = RequestPair { request_id: 1337, message: vec![5u8] };

        let got = RequestPair::<Vec<u8>>::decode(&mut &*raw_pair).unwrap();
        assert_eq!(expected.length(), raw_pair.len());
        assert_eq!(expected, got);
    }
}
