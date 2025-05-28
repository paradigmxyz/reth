//! Implements Ress protocol
//! Defines structs/enums for messages, request-response pairs.
//!
//! Examples include creating, encoding, and decoding protocol messages.

use crate::NodeType;
use alloy_consensus::Header;
use alloy_primitives::{
    B256, BlockHash, Bytes,
    bytes::{Buf, BufMut},
};
use alloy_rlp::{BytesMut, Decodable, Encodable, RlpDecodable, RlpEncodable};
use reth_eth_wire::{Capability, message::RequestPair, protocol::Protocol};
use reth_ethereum_primitives::BlockBody;

/// An Ress protocol message, containing a message ID and payload.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct ZkRessProtocolMessage {
    /// The unique identifier representing the type of the Ress message.
    pub message_type: ZkRessMessageID,
    /// The content of the message, including specific data based on the message type.
    pub message: ZkRessMessage,
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for ZkRessProtocolMessage {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let message: ZkRessMessage = u.arbitrary()?;
        Ok(Self { message_type: message.message_id(), message })
    }
}

impl ZkRessProtocolMessage {
    /// Returns the capability for the flavor of `zkress` protocol.
    pub const fn capability(name: &'static str, version: usize) -> Capability {
        Capability::new_static(name, version)
    }

    /// Returns the protocol for the flavor of `zkress` protocol.
    pub const fn protocol(name: &'static str, version: usize) -> Protocol {
        Protocol::new(Self::capability(name, version), 7)
    }

    /// Create node type message.
    pub const fn node_type(node_type: NodeType) -> Self {
        ZkRessMessage::NodeType(node_type).into_protocol_message()
    }

    /// Headers request.
    pub const fn get_headers(request_id: u64, request: GetHeaders) -> Self {
        ZkRessMessage::GetHeaders(RequestPair { request_id, message: request })
            .into_protocol_message()
    }

    /// Headers response.
    pub const fn headers(request_id: u64, headers: Vec<Header>) -> Self {
        ZkRessMessage::Headers(RequestPair { request_id, message: headers }).into_protocol_message()
    }

    /// Block bodies request.
    pub const fn get_block_bodies(request_id: u64, block_hashes: Vec<B256>) -> Self {
        ZkRessMessage::GetBlockBodies(RequestPair { request_id, message: block_hashes })
            .into_protocol_message()
    }

    /// Block bodies response.
    pub const fn block_bodies(request_id: u64, bodies: Vec<BlockBody>) -> Self {
        ZkRessMessage::BlockBodies(RequestPair { request_id, message: bodies })
            .into_protocol_message()
    }

    /// Execution witness request.
    pub const fn get_witness(request_id: u64, block_hash: BlockHash) -> Self {
        ZkRessMessage::GetWitness(RequestPair { request_id, message: block_hash })
            .into_protocol_message()
    }

    /// Execution witness response.
    pub const fn witness(request_id: u64, witness: Vec<Bytes>) -> Self {
        ZkRessMessage::Witness(RequestPair { request_id, message: witness }).into_protocol_message()
    }

    /// Return RLP encoded message.
    pub fn encoded(&self) -> BytesMut {
        let mut buf = BytesMut::with_capacity(self.length());
        self.encode(&mut buf);
        buf
    }

    /// Decodes a `RessProtocolMessage` from the given message buffer.
    pub fn decode_message(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let message_type = ZkRessMessageID::decode(buf)?;
        let message = match message_type {
            ZkRessMessageID::NodeType => ZkRessMessage::NodeType(NodeType::decode(buf)?),
            ZkRessMessageID::GetHeaders => ZkRessMessage::GetHeaders(RequestPair::decode(buf)?),
            ZkRessMessageID::Headers => ZkRessMessage::Headers(RequestPair::decode(buf)?),
            ZkRessMessageID::GetBlockBodies => {
                ZkRessMessage::GetBlockBodies(RequestPair::decode(buf)?)
            }
            ZkRessMessageID::BlockBodies => ZkRessMessage::BlockBodies(RequestPair::decode(buf)?),
            ZkRessMessageID::GetWitness => ZkRessMessage::GetWitness(RequestPair::decode(buf)?),
            ZkRessMessageID::Witness => ZkRessMessage::Witness(RequestPair::decode(buf)?),
        };
        Ok(Self { message_type, message })
    }
}

impl Encodable for ZkRessProtocolMessage {
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
pub enum ZkRessMessageID {
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

    /// Witness request message.
    GetWitness = 0x05,
    /// Witness response message.
    Witness = 0x06,
}

impl Encodable for ZkRessMessageID {
    fn encode(&self, out: &mut dyn BufMut) {
        out.put_u8(*self as u8);
    }

    fn length(&self) -> usize {
        1
    }
}

impl Decodable for ZkRessMessageID {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let id = match buf.first().ok_or(alloy_rlp::Error::InputTooShort)? {
            0x00 => Self::NodeType,
            0x01 => Self::GetHeaders,
            0x02 => Self::Headers,
            0x03 => Self::GetBlockBodies,
            0x04 => Self::BlockBodies,
            0x05 => Self::GetWitness,
            0x06 => Self::Witness,
            _ => return Err(alloy_rlp::Error::Custom("Invalid message type")),
        };
        buf.advance(1);
        Ok(id)
    }
}

/// Represents a message in the ress protocol.
#[derive(PartialEq, Eq, Clone, Debug)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub enum ZkRessMessage {
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

    /// Represents a witness request message.
    GetWitness(RequestPair<BlockHash>),
    /// Represents a witness response message.
    Witness(RequestPair<Vec<Bytes>>),
}

impl ZkRessMessage {
    /// Return [`ZkRessMessageID`] that corresponds to the given message.
    pub const fn message_id(&self) -> ZkRessMessageID {
        match self {
            Self::NodeType(_) => ZkRessMessageID::NodeType,
            Self::GetHeaders(_) => ZkRessMessageID::GetHeaders,
            Self::Headers(_) => ZkRessMessageID::Headers,
            Self::GetBlockBodies(_) => ZkRessMessageID::GetBlockBodies,
            Self::BlockBodies(_) => ZkRessMessageID::BlockBodies,
            Self::GetWitness(_) => ZkRessMessageID::GetWitness,
            Self::Witness(_) => ZkRessMessageID::Witness,
        }
    }

    /// Convert message into [`ZkRessProtocolMessage`].
    pub const fn into_protocol_message(self) -> ZkRessProtocolMessage {
        let message_type = self.message_id();
        ZkRessProtocolMessage { message_type, message: self }
    }
}

impl From<ZkRessMessage> for ZkRessProtocolMessage {
    fn from(value: ZkRessMessage) -> Self {
        value.into_protocol_message()
    }
}

impl Encodable for ZkRessMessage {
    fn encode(&self, out: &mut dyn BufMut) {
        match self {
            Self::NodeType(node_type) => node_type.encode(out),
            Self::GetHeaders(request) => request.encode(out),
            Self::Headers(header) => header.encode(out),
            Self::GetBlockBodies(request) => request.encode(out),
            Self::BlockBodies(body) => body.encode(out),
            Self::GetWitness(request) => request.encode(out),
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
            Self::GetWitness(request) => request.length(),
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
        let protocol = ZkRessProtocolMessage::protocol("zkress", 1);
        assert_eq!(protocol.messages(), ZkRessMessageID::COUNT as u8);
    }

    proptest! {
        #[test]
        fn message_type_roundtrip(message_type in arb::<ZkRessMessageID>()) {
            rlp_roundtrip(message_type);
        }

        #[test]
        fn message_roundtrip(message in arb::<ZkRessProtocolMessage>()) {
            let encoded = alloy_rlp::encode(&message);
            let decoded = ZkRessProtocolMessage::decode_message(&mut &encoded[..]);
            assert_eq!(Ok(message), decoded);
        }
    }
}
