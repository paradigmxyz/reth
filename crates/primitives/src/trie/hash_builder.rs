use super::TrieMask;
use crate::H256;
use reth_codecs::{main_codec, Compact};

/// The hash builder state for storing in the database.
/// Check the `reth-trie` crate for more info on hash builder.
#[main_codec]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HashBuilderState {
    /// The current key.
    pub key: Vec<u8>,
    /// The builder stack.
    pub stack: Vec<Vec<u8>>,

    /// Group masks.
    pub groups: Vec<TrieMask>,
    /// Tree masks.
    pub tree_masks: Vec<TrieMask>,
    /// Hash masks.
    pub hash_masks: Vec<TrieMask>,

    /// Flag indicating if the current node is stored in the database.
    pub stored_in_database: bool,

    /// The current node value.
    pub value: HashBuilderValue,
}

/// The current value of the hash builder.
#[main_codec]
#[derive(Clone, PartialEq)]
pub enum HashBuilderValue {
    /// Value of the leaf node.
    Hash(H256),
    /// Hash of adjacent nodes.
    Bytes(Vec<u8>),
}

impl std::fmt::Debug for HashBuilderValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes(bytes) => write!(f, "Bytes({:?})", hex::encode(bytes)),
            Self::Hash(hash) => write!(f, "Hash({:?})", hash),
        }
    }
}

impl From<Vec<u8>> for HashBuilderValue {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<&[u8]> for HashBuilderValue {
    fn from(value: &[u8]) -> Self {
        Self::Bytes(value.to_vec())
    }
}

impl From<H256> for HashBuilderValue {
    fn from(value: H256) -> Self {
        Self::Hash(value)
    }
}

impl Default for HashBuilderValue {
    fn default() -> Self {
        Self::Bytes(vec![])
    }
}
