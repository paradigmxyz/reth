use crate::{trie_cursor::CursorSubNode, updates::TrieUpdates};
use reth_primitives::{
    stage::MerkleCheckpoint,
    trie::{hash_builder::HashBuilder, Nibbles},
    H256,
};

/// The progress of the state root computation.
#[derive(Debug)]
pub enum StateRootProgress {
    /// The complete state root computation with updates and computed root.
    Complete(H256, TrieUpdates),
    /// The intermediate progress of state root computation.
    /// Contains the walker stack, the hash builder and the trie updates.
    Progress(Box<IntermediateStateRootState>, TrieUpdates),
}

/// The intermediate state of the state root computation.
#[derive(Debug)]
pub struct IntermediateStateRootState {
    /// Previously constructed hash builder.
    pub hash_builder: HashBuilder,
    /// Previously recorded walker stack.
    pub walker_stack: Vec<CursorSubNode>,
    /// The last hashed account key processed.
    pub last_account_key: H256,
    /// The last walker key processed.
    pub last_walker_key: Nibbles,
}

impl From<MerkleCheckpoint> for IntermediateStateRootState {
    fn from(value: MerkleCheckpoint) -> Self {
        Self {
            hash_builder: HashBuilder::from(value.state),
            walker_stack: value.walker_stack.into_iter().map(CursorSubNode::from).collect(),
            last_account_key: value.last_account_key,
            last_walker_key: Nibbles::from(value.last_walker_key),
        }
    }
}
