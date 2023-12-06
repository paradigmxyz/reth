use generic_array::GenericArray;
use reth_primitives::{keccak256, NodeRecord, PeerId};

/// The key type for the table.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) struct NodeKey(pub(crate) PeerId);

impl From<PeerId> for NodeKey {
    fn from(value: PeerId) -> Self {
        NodeKey(value)
    }
}

impl From<NodeKey> for discv5::Key<NodeKey> {
    fn from(value: NodeKey) -> Self {
        let hash = keccak256(value.0.as_slice());
        let hash = *GenericArray::from_slice(hash.as_slice());
        discv5::Key::new_raw(value, hash)
    }
}

impl From<&NodeRecord> for NodeKey {
    fn from(node: &NodeRecord) -> Self {
        NodeKey(node.id)
    }
}

/// Converts a `PeerId` into the required `Key` type for the table
#[inline]
pub(crate) fn kad_key(node: PeerId) -> discv5::Key<NodeKey> {
    discv5::kbucket::Key::from(NodeKey::from(node))
}
