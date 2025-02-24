use alloy_primitives::{
    bytes::{Buf, BufMut},
    Bytes, B256,
};
use alloy_rlp::{
    Decodable, Encodable, RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper,
};

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

/// State witness entry in the format used for networking.
#[derive(PartialEq, Eq, Clone, Debug, RlpEncodable, RlpDecodable)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub struct StateWitnessEntry {
    /// Trie node hash.
    pub hash: B256,
    /// RLP-encoded trie node.
    pub bytes: Bytes,
}

/// State witness in the format used for networking.
#[derive(PartialEq, Eq, Clone, Default, Debug, RlpEncodableWrapper, RlpDecodableWrapper)]
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
pub struct StateWitnessNet(
    /// State witness entries.
    pub Vec<StateWitnessEntry>,
);

impl FromIterator<(B256, Bytes)> for StateWitnessNet {
    fn from_iter<T: IntoIterator<Item = (B256, Bytes)>>(iter: T) -> Self {
        Self(Vec::from_iter(
            iter.into_iter().map(|(hash, bytes)| StateWitnessEntry { hash, bytes }),
        ))
    }
}
