//! Implements Ress protocol
//! Defines structs/enums for messages, request-response pairs.
//!
//! Examples include creating, encoding, and decoding protocol messages.

use crate::NodeType;
use alloy_consensus::Header;
use alloy_primitives::{
    bytes::{Buf, BufMut},
    BlockHash, Bytes, B256,
};
use alloy_rlp::{BytesMut, Decodable, Encodable, RlpDecodable, RlpEncodable};
use reth_eth_wire::{message::RequestPair, protocol::Protocol, Capability};
use reth_ethereum_primitives::BlockBody;

/// An Ress protocol message, containing a message ID and payload.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct RessProtocolMessage {
    /// The unique identifier representing the type of the Ress message.
    pub message_type: RessMessageID,
    /// The content of the message, including specific data based on the message type.
    pub message: RessMessage,
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for RessProtocolMessage {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let message: RessMessage = u.arbitrary()?;
        Ok(Self { message_type: message.message_id(), message })
    }
}

impl RessProtocolMessage {
    /// Returns the capability for the `ress` protocol.
    pub fn capability() -> Capability {
        Capability::new_static("ress", 1)
    }

    /// Returns the protocol for the `ress` protocol.
    pub fn protocol() -> Protocol {
        Protocol::new(Self::capability(), 9)
    }

    /// Create node type message.
    pub fn node_type(node_type: NodeType) -> Self {
        RessMessage::NodeType(node_type).into_protocol_message()
    }

    /// Headers request.
    pub fn get_headers(request_id: u64, request: GetHeaders) -> Self {
        RessMessage::GetHeaders(RequestPair { request_id, message: request })
            .into_protocol_message()
    }

    /// Headers response.
    pub fn headers(request_id: u64, headers: Vec<Header>) -> Self {
        RessMessage::Headers(RequestPair { request_id, message: headers }).into_protocol_message()
    }

    /// Block bodies request.
    pub fn get_block_bodies(request_id: u64, block_hashes: Vec<B256>) -> Self {
        RessMessage::GetBlockBodies(RequestPair { request_id, message: block_hashes })
            .into_protocol_message()
    }

    /// Block bodies response.
    pub fn block_bodies(request_id: u64, bodies: Vec<BlockBody>) -> Self {
        RessMessage::BlockBodies(RequestPair { request_id, message: bodies })
            .into_protocol_message()
    }

    /// Bytecode request.
    pub fn get_bytecode(request_id: u64, code_hash: B256) -> Self {
        RessMessage::GetBytecode(RequestPair { request_id, message: code_hash })
            .into_protocol_message()
    }

    /// Bytecode response.
    pub fn bytecode(request_id: u64, bytecode: Bytes) -> Self {
        RessMessage::Bytecode(RequestPair { request_id, message: bytecode }).into_protocol_message()
    }

    /// Execution witness request.
    pub fn get_witness(request_id: u64, block_hash: BlockHash) -> Self {
        RessMessage::GetWitness(RequestPair { request_id, message: block_hash })
            .into_protocol_message()
    }

    /// Execution witness response.
    pub fn witness(request_id: u64, witness: Vec<Bytes>) -> Self {
        RessMessage::Witness(RequestPair { request_id, message: witness }).into_protocol_message()
    }

    /// Return RLP encoded message.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::with_capacity(self.length());
        self.encode(&mut buf);
        buf
    }

    /// Decodes a `RessProtocolMessage` from the given message buffer.
    pub fn decode_message(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let message_type = RessMessageID::decode(buf)?;
        let message = match message_type {
            RessMessageID::NodeType => RessMessage::NodeType(NodeType::decode(buf)?),
            RessMessageID::GetHeaders => RessMessage::GetHeaders(RequestPair::decode(buf)?),
            RessMessageID::Headers => RessMessage::Headers(RequestPair::decode(buf)?),
            RessMessageID::GetBlockBodies => RessMessage::GetBlockBodies(RequestPair::decode(buf)?),
            RessMessageID::BlockBodies => RessMessage::BlockBodies(RequestPair::decode(buf)?),
            RessMessageID::GetBytecode => RessMessage::GetBytecode(RequestPair::decode(buf)?),
            RessMessageID::Bytecode => RessMessage::Bytecode(RequestPair::decode(buf)?),
            RessMessageID::GetWitness => RessMessage::GetWitness(RequestPair::decode(buf)?),
            RessMessageID::Witness => RessMessage::Witness(RequestPair::decode(buf)?),
        };
        Ok(Self { message_type, message })
    }
}

impl Encodable for RessProtocolMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        self.message_type.encode(out);
        self.message.encode(out);
    }

    fn length(&self) -> usize {
        self.message_type.length() + self.message.length()
    }
}

/// Represents message IDs for `ress` protocol messages.
#[repr(u8)]
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[cfg_attr(test, derive(strum_macros::EnumCount))]
pub enum RessMessageID {
    /// Node type message.
    NodeType = 0x00,

    /// Headers request message.
    GetHeaders = 0x01,
    /// Headers response message.
    Headers = 0x02,

    /// Block bodies request message.
    GetBlockBodies = 0x03,
    /// Block bodies response message.
    BlockBodies = 0x04,

    /// Bytecode request message.
    GetBytecode = 0x05,
    /// Bytecode response message.
    Bytecode = 0x06,

    /// Witness request message.
    GetWitness = 0x07,
    /// Witness response message.
    Witness = 0x08,
}

impl Encodable for RessMessageID {
    fn encode(&self, out: &mut dyn BufMut) {
        out.put_u8(*self as u8);
    }

    fn length(&self) -> usize {
        1
    }
}

impl Decodable for RessMessageID {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let id = match buf.first().ok_or(alloy_rlp::Error::InputTooShort)? {
            0x00 => Self::NodeType,
            0x01 => Self::GetHeaders,
            0x02 => Self::Headers,
            0x03 => Self::GetBlockBodies,
            0x04 => Self::BlockBodies,
            0x05 => Self::GetBytecode,
            0x06 => Self::Bytecode,
            0x07 => Self::GetWitness,
            0x08 => Self::Witness,
            _ => return Err(alloy_rlp::Error::Custom("Invalid message type")),
        };
        buf.advance(1);
        Ok(id)
    }
}

/// Represents a message in the ress protocol.
#[derive(PartialEq, Eq, Clone, Debug)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub enum RessMessage {
    /// Represents a node type message required for handshake.
    NodeType(NodeType),

    /// Represents a headers request message.
    GetHeaders(RequestPair<GetHeaders>),
    /// Represents a headers response message.
    Headers(RequestPair<Vec<Header>>),

    /// Represents a block bodies request message.
    GetBlockBodies(RequestPair<Vec<B256>>),
    /// Represents a block bodies response message.
    BlockBodies(RequestPair<Vec<BlockBody>>),

    /// Represents a bytecode request message.
    GetBytecode(RequestPair<B256>),
    /// Represents a bytecode response message.
    Bytecode(RequestPair<Bytes>),

    /// Represents a witness request message.
    GetWitness(RequestPair<BlockHash>),
    /// Represents a witness response message.
    Witness(RequestPair<Vec<Bytes>>),
}

impl RessMessage {
    /// Return [`RessMessageID`] that corresponds to the given message.
    pub fn message_id(&self) -> RessMessageID {
        match self {
            Self::NodeType(_) => RessMessageID::NodeType,
            Self::GetHeaders(_) => RessMessageID::GetHeaders,
            Self::Headers(_) => RessMessageID::Headers,
            Self::GetBlockBodies(_) => RessMessageID::GetBlockBodies,
            Self::BlockBodies(_) => RessMessageID::BlockBodies,
            Self::GetBytecode(_) => RessMessageID::GetBytecode,
            Self::Bytecode(_) => RessMessageID::Bytecode,
            Self::GetWitness(_) => RessMessageID::GetWitness,
            Self::Witness(_) => RessMessageID::Witness,
        }
    }

    /// Convert message into [`RessProtocolMessage`].
    pub fn into_protocol_message(self) -> RessProtocolMessage {
        let message_type = self.message_id();
        RessProtocolMessage { message_type, message: self }
    }
}

impl From<RessMessage> for RessProtocolMessage {
    fn from(value: RessMessage) -> Self {
        value.into_protocol_message()
    }
}

impl Encodable for RessMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::NodeType(node_type) => node_type.encode(out),
            Self::GetHeaders(request) => request.encode(out),
            Self::Headers(header) => header.encode(out),
            Self::GetBlockBodies(request) => request.encode(out),
            Self::BlockBodies(body) => body.encode(out),
            Self::GetBytecode(request) | Self::GetWitness(request) => request.encode(out),
            Self::Bytecode(bytecode) => bytecode.encode(out),
            Self::Witness(witness) => witness.encode(out),
        }
    }

    fn length(&self) -> usize {
        match self {
            Self::NodeType(node_type) => node_type.length(),
            Self::GetHeaders(request) => request.length(),
            Self::Headers(header) => header.length(),
            Self::GetBlockBodies(request) => request.length(),
            Self::BlockBodies(body) => body.length(),
            Self::GetBytecode(request) | Self::GetWitness(request) => request.length(),
            Self::Bytecode(bytecode) => bytecode.length(),
            Self::Witness(witness) => witness.length(),
        }
    }
}

/// A request for a peer to return block headers starting at the requested block.
/// The peer must return at most [`limit`](#structfield.limit) headers.
/// The headers will be returned starting at [`start_hash`](#structfield.start_hash), traversing
/// towards the genesis block.
#[derive(PartialEq, Eq, Clone, Copy, Debug, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub struct GetHeaders {
    /// The block hash that the peer should start returning headers from.
    pub start_hash: BlockHash,

    /// The maximum number of headers to return.
    pub limit: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest_arbitrary_interop::arb;
    use std::fmt;
    use strum::EnumCount;

    fn rlp_roundtrip<V>(value: V)
    where
        V: Encodable + Decodable + PartialEq + fmt::Debug,
    {
        let encoded = alloy_rlp::encode(&value);
        let decoded = V::decode(&mut &encoded[..]);
        assert_eq!(Ok(value), decoded);
    }

    #[test]
    fn protocol_message_count() {
        let protocol = RessProtocolMessage::protocol();
        assert_eq!(protocol.messages(), RessMessageID::COUNT as u8);
    }

    proptest! {
        #[test]
        fn message_type_roundtrip(message_type in arb::<RessMessageID>()) {
            rlp_roundtrip(message_type);
        }

        #[test]
        fn message_roundtrip(message in arb::<RessProtocolMessage>()) {
            let encoded = alloy_rlp::encode(&message);
            let decoded = RessProtocolMessage::decode_message(&mut &encoded[..]);
            assert_eq!(Ok(message), decoded);
        }
    }
}
