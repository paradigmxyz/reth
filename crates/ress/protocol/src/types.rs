use alloy_primitives::bytes::{Buf, BufMut};
use alloy_rlp::{Decodable, Encodable};

/// Node type variant.
#[repr(u8)]
#[derive(PartialEq, Eq, Copy, Clone, Debug)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub enum NodeType {
    /// Stateless ress node.
    Stateless = 0x00,
    /// Stateful reth node.
    Stateful,
}

impl Encodable for NodeType {
    fn encode(&self, out: &mut dyn BufMut) {
        out.put_u8(*self as u8);
    }

    fn length(&self) -> usize {
        1
    }
}

impl Decodable for NodeType {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let id = match buf.first().ok_or(alloy_rlp::Error::InputTooShort)? {
            0x00 => Self::Stateless,
            0x01 => Self::Stateful,
            _ => return Err(alloy_rlp::Error::Custom("Invalid message type")),
        };
        buf.advance(1);
        Ok(id)
    }
}

impl NodeType {
    /// Return `true` if node type is stateful.
    pub fn is_stateful(&self) -> bool {
        matches!(self, Self::Stateful)
    }

    /// Return `true` if the connection between this and other node types
    /// can be considered valid.
    ///
    /// Validity:
    ///           | stateless | stateful |
    /// ----------|-----------|----------|
    /// stateless |     +     |     +    |
    /// stateful  |     +     |     -    |
    pub fn is_valid_connection(&self, other: &Self) -> bool {
        !self.is_stateful() || !other.is_stateful()
    }
}
