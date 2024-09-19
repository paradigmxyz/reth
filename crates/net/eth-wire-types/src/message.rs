//! Implements Ethereum wire protocol for versions 66, 67, and 68.
//! Defines structs/enums for messages, request-response pairs, and broadcasts.
//! Handles compatibility with [`EthVersion`].
//!
//! Examples include creating, encoding, and decoding protocol messages.
//!
//! Reference: [Ethereum Wire Protocol](https://github.com/ethereum/wiki/wiki/Ethereum-Wire-Protocol).

use super::{
    broadcast::NewBlockHashes, BlockBodies, BlockHeaders, GetBlockBodies, GetBlockHeaders,
    GetNodeData, GetPooledTransactions, GetReceipts, NewBlock, NewPooledTransactionHashes66,
    NewPooledTransactionHashes68, NodeData, PooledTransactions, Receipts, Status, Transactions,
};
use crate::{EthVersion, SharedTransactions};

use alloy_primitives::bytes::{Buf, BufMut};
use alloy_rlp::{length_of_length, Decodable, Encodable, Header};
use std::{fmt::Debug, sync::Arc};

/// [`MAX_MESSAGE_SIZE`] is the maximum cap on the size of a protocol message.
// https://github.com/ethereum/go-ethereum/blob/30602163d5d8321fbc68afdcbbaf2362b2641bde/eth/protocols/eth/protocol.go#L50
pub const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Error when sending/receiving a message
#[derive(thiserror::Error, Debug)]
pub enum MessageError {
    /// Flags an unrecognized message ID for a given protocol version.
    #[error("message id {1:?} is invalid for version {0:?}")]
    Invalid(EthVersion, EthMessageID),
    /// Thrown when rlp decoding a message message failed.
    #[error("RLP error: {0}")]
    RlpError(#[from] alloy_rlp::Error),
}

/// An `eth` protocol message, containing a message ID and payload.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProtocolMessage {
    /// The unique identifier representing the type of the Ethereum message.
    pub message_type: EthMessageID,
    /// The content of the message, including specific data based on the message type.
    pub message: EthMessage,
}

impl ProtocolMessage {
    /// Create a new `ProtocolMessage` from a message type and message rlp bytes.
    pub fn decode_message(version: EthVersion, buf: &mut &[u8]) -> Result<Self, MessageError> {
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
                    return Err(MessageError::Invalid(version, EthMessageID::GetNodeData))
                }
                let request_pair = RequestPair::<GetNodeData>::decode(buf)?;
                EthMessage::GetNodeData(request_pair)
            }
            EthMessageID::NodeData => {
                if version >= EthVersion::Eth67 {
                    return Err(MessageError::Invalid(version, EthMessageID::GetNodeData))
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
        Ok(Self { message_type, message })
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
        Self { message_type: message.message_id(), message }
    }
}

/// Represents messages that can be sent to multiple peers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolBroadcastMessage {
    /// The unique identifier representing the type of the Ethereum message.
    pub message_type: EthMessageID,
    /// The content of the message to be broadcasted, including specific data based on the message
    /// type.
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
        Self { message_type: message.message_id(), message }
    }
}

/// Represents a message in the eth wire protocol, versions 66, 67 and 68.
///
/// The ethereum wire protocol is a set of messages that are broadcast to the network in two
/// styles:
///  * A request message sent by a peer (such as [`GetPooledTransactions`]), and an associated
///    response message (such as [`PooledTransactions`]).
///  * A message that is broadcast to the network, without a corresponding request.
///
/// The newer `eth/66` is an efficiency upgrade on top of `eth/65`, introducing a request id to
/// correlate request-response message pairs. This allows for request multiplexing.
///
/// The `eth/67` is based on `eth/66` but only removes two messages, [`GetNodeData`] and
/// [`NodeData`].
///
/// The `eth/68` changes only `NewPooledTransactionHashes` to include `types` and `sized`. For
/// it, `NewPooledTransactionHashes` is renamed as [`NewPooledTransactionHashes66`] and
/// [`NewPooledTransactionHashes68`] is defined.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EthMessage {
    /// Represents a Status message required for the protocol handshake.
    Status(Status),
    /// Represents a `NewBlockHashes` message broadcast to the network.
    NewBlockHashes(NewBlockHashes),
    /// Represents a `NewBlock` message broadcast to the network.
    NewBlock(Box<NewBlock>),
    /// Represents a Transactions message broadcast to the network.
    Transactions(Transactions),
    /// Represents a `NewPooledTransactionHashes` message for eth/66 version.
    NewPooledTransactionHashes66(NewPooledTransactionHashes66),
    /// Represents a `NewPooledTransactionHashes` message for eth/68 version.
    NewPooledTransactionHashes68(NewPooledTransactionHashes68),
    // The following messages are request-response message pairs
    /// Represents a `GetBlockHeaders` request-response pair.
    GetBlockHeaders(RequestPair<GetBlockHeaders>),
    /// Represents a `BlockHeaders` request-response pair.
    BlockHeaders(RequestPair<BlockHeaders>),
    /// Represents a `GetBlockBodies` request-response pair.
    GetBlockBodies(RequestPair<GetBlockBodies>),
    /// Represents a `BlockBodies` request-response pair.
    BlockBodies(RequestPair<BlockBodies>),
    /// Represents a `GetPooledTransactions` request-response pair.
    GetPooledTransactions(RequestPair<GetPooledTransactions>),
    /// Represents a `PooledTransactions` request-response pair.
    PooledTransactions(RequestPair<PooledTransactions>),
    /// Represents a `GetNodeData` request-response pair.
    GetNodeData(RequestPair<GetNodeData>),
    /// Represents a `NodeData` request-response pair.
    NodeData(RequestPair<NodeData>),
    /// Represents a `GetReceipts` request-response pair.
    GetReceipts(RequestPair<GetReceipts>),
    /// Represents a Receipts request-response pair.
    Receipts(RequestPair<Receipts>),
}

impl EthMessage {
    /// Returns the message's ID.
    pub const fn message_id(&self) -> EthMessageID {
        match self {
            Self::Status(_) => EthMessageID::Status,
            Self::NewBlockHashes(_) => EthMessageID::NewBlockHashes,
            Self::NewBlock(_) => EthMessageID::NewBlock,
            Self::Transactions(_) => EthMessageID::Transactions,
            Self::NewPooledTransactionHashes66(_) | Self::NewPooledTransactionHashes68(_) => {
                EthMessageID::NewPooledTransactionHashes
            }
            Self::GetBlockHeaders(_) => EthMessageID::GetBlockHeaders,
            Self::BlockHeaders(_) => EthMessageID::BlockHeaders,
            Self::GetBlockBodies(_) => EthMessageID::GetBlockBodies,
            Self::BlockBodies(_) => EthMessageID::BlockBodies,
            Self::GetPooledTransactions(_) => EthMessageID::GetPooledTransactions,
            Self::PooledTransactions(_) => EthMessageID::PooledTransactions,
            Self::GetNodeData(_) => EthMessageID::GetNodeData,
            Self::NodeData(_) => EthMessageID::NodeData,
            Self::GetReceipts(_) => EthMessageID::GetReceipts,
            Self::Receipts(_) => EthMessageID::Receipts,
        }
    }
}

impl Encodable for EthMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::Status(status) => status.encode(out),
            Self::NewBlockHashes(new_block_hashes) => new_block_hashes.encode(out),
            Self::NewBlock(new_block) => new_block.encode(out),
            Self::Transactions(transactions) => transactions.encode(out),
            Self::NewPooledTransactionHashes66(hashes) => hashes.encode(out),
            Self::NewPooledTransactionHashes68(hashes) => hashes.encode(out),
            Self::GetBlockHeaders(request) => request.encode(out),
            Self::BlockHeaders(headers) => headers.encode(out),
            Self::GetBlockBodies(request) => request.encode(out),
            Self::BlockBodies(bodies) => bodies.encode(out),
            Self::GetPooledTransactions(request) => request.encode(out),
            Self::PooledTransactions(transactions) => transactions.encode(out),
            Self::GetNodeData(request) => request.encode(out),
            Self::NodeData(data) => data.encode(out),
            Self::GetReceipts(request) => request.encode(out),
            Self::Receipts(receipts) => receipts.encode(out),
        }
    }
    fn length(&self) -> usize {
        match self {
            Self::Status(status) => status.length(),
            Self::NewBlockHashes(new_block_hashes) => new_block_hashes.length(),
            Self::NewBlock(new_block) => new_block.length(),
            Self::Transactions(transactions) => transactions.length(),
            Self::NewPooledTransactionHashes66(hashes) => hashes.length(),
            Self::NewPooledTransactionHashes68(hashes) => hashes.length(),
            Self::GetBlockHeaders(request) => request.length(),
            Self::BlockHeaders(headers) => headers.length(),
            Self::GetBlockBodies(request) => request.length(),
            Self::BlockBodies(bodies) => bodies.length(),
            Self::GetPooledTransactions(request) => request.length(),
            Self::PooledTransactions(transactions) => transactions.length(),
            Self::GetNodeData(request) => request.length(),
            Self::NodeData(data) => data.length(),
            Self::GetReceipts(request) => request.length(),
            Self::Receipts(receipts) => receipts.length(),
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
    /// Represents a new block broadcast message.
    NewBlock(Arc<NewBlock>),
    /// Represents a transactions broadcast message.
    Transactions(SharedTransactions),
}

// === impl EthBroadcastMessage ===

impl EthBroadcastMessage {
    /// Returns the message's ID.
    pub const fn message_id(&self) -> EthMessageID {
        match self {
            Self::NewBlock(_) => EthMessageID::NewBlock,
            Self::Transactions(_) => EthMessageID::Transactions,
        }
    }
}

impl Encodable for EthBroadcastMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::NewBlock(new_block) => new_block.encode(out),
            Self::Transactions(transactions) => transactions.encode(out),
        }
    }

    fn length(&self) -> usize {
        match self {
            Self::NewBlock(new_block) => new_block.length(),
            Self::Transactions(transactions) => transactions.length(),
        }
    }
}

/// Represents message IDs for eth protocol messages.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EthMessageID {
    /// Status message.
    Status = 0x00,
    /// New block hashes message.
    NewBlockHashes = 0x01,
    /// Transactions message.
    Transactions = 0x02,
    /// Get block headers message.
    GetBlockHeaders = 0x03,
    /// Block headers message.
    BlockHeaders = 0x04,
    /// Get block bodies message.
    GetBlockBodies = 0x05,
    /// Block bodies message.
    BlockBodies = 0x06,
    /// New block message.
    NewBlock = 0x07,
    /// New pooled transaction hashes message.
    NewPooledTransactionHashes = 0x08,
    /// Requests pooled transactions.
    GetPooledTransactions = 0x09,
    /// Represents pooled transactions.
    PooledTransactions = 0x0a,
    /// Requests node data.
    GetNodeData = 0x0d,
    /// Represents node data.
    NodeData = 0x0e,
    /// Requests receipts.
    GetReceipts = 0x0f,
    /// Represents receipts.
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
        let id = match buf.first().ok_or(alloy_rlp::Error::InputTooShort)? {
            0x00 => Self::Status,
            0x01 => Self::NewBlockHashes,
            0x02 => Self::Transactions,
            0x03 => Self::GetBlockHeaders,
            0x04 => Self::BlockHeaders,
            0x05 => Self::GetBlockBodies,
            0x06 => Self::BlockBodies,
            0x07 => Self::NewBlock,
            0x08 => Self::NewPooledTransactionHashes,
            0x09 => Self::GetPooledTransactions,
            0x0a => Self::PooledTransactions,
            0x0d => Self::GetNodeData,
            0x0e => Self::NodeData,
            0x0f => Self::GetReceipts,
            0x10 => Self::Receipts,
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
            0x00 => Ok(Self::Status),
            0x01 => Ok(Self::NewBlockHashes),
            0x02 => Ok(Self::Transactions),
            0x03 => Ok(Self::GetBlockHeaders),
            0x04 => Ok(Self::BlockHeaders),
            0x05 => Ok(Self::GetBlockBodies),
            0x06 => Ok(Self::BlockBodies),
            0x07 => Ok(Self::NewBlock),
            0x08 => Ok(Self::NewPooledTransactionHashes),
            0x09 => Ok(Self::GetPooledTransactions),
            0x0a => Ok(Self::PooledTransactions),
            0x0d => Ok(Self::GetNodeData),
            0x0e => Ok(Self::NodeData),
            0x0f => Ok(Self::GetReceipts),
            0x10 => Ok(Self::Receipts),
            _ => Err("Invalid message ID"),
        }
    }
}

/// This is used for all request-response style `eth` protocol messages.
/// This can represent either a request or a response, since both include a message payload and
/// request id.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
        let header = Header::decode(buf)?;

        let initial_length = buf.len();
        let request_id = u64::decode(buf)?;
        let message = T::decode(buf)?;

        // Check that the buffer consumed exactly payload_length bytes after decoding the
        // RequestPair
        let consumed_len = initial_length - buf.len();
        if consumed_len != header.payload_length {
            return Err(alloy_rlp::Error::UnexpectedLength)
        }

        Ok(Self { request_id, message })
    }
}

#[cfg(test)]
mod tests {
    use super::MessageError;
    use crate::{
        message::RequestPair, EthMessage, EthMessageID, GetNodeData, NodeData, ProtocolMessage,
    };
    use alloy_primitives::hex;
    use alloy_rlp::{Decodable, Encodable, Error};

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
        assert!(matches!(msg, Err(MessageError::Invalid(..))));

        let node_data =
            EthMessage::NodeData(RequestPair { request_id: 1337, message: NodeData(vec![]) });
        let buf =
            encode(ProtocolMessage { message_type: EthMessageID::NodeData, message: node_data });
        let msg = ProtocolMessage::decode_message(crate::EthVersion::Eth67, &mut &buf[..]);
        assert!(matches!(msg, Err(MessageError::Invalid(..))));
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

    #[test]
    fn malicious_request_pair_decode() {
        // A maliciously encoded request pair, where the len(full_list) is 5, but it
        // actually consumes 6 bytes when decoding
        //
        // c5: start of list (c0) + len(full_list) (length is <55 bytes)
        // 82: 0x80 + len(1337)
        // 05 39: 1337 (request_id)
        // === full_list ===
        // c2: start of list (c0) + len(list) (length is <55 bytes)
        // 05 05: 5 5(message)
        let raw_pair = &hex!("c5820539c20505")[..];

        let result = RequestPair::<Vec<u8>>::decode(&mut &*raw_pair);
        assert!(matches!(result, Err(Error::UnexpectedLength)));
    }
}
