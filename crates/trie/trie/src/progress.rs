use crate::{trie_cursor::CursorSubNode, updates::TrieUpdates};
use reth_primitives::{stage::MerkleCheckpoint, trie::hash_builder::HashBuilder, B256};

/// The progress of the state root computation.
#[derive(Debug)]
pub enum StateRootProgress {
    /// The complete state root computation with updates and computed root.
    Complete(B256, usize, TrieUpdates),
    /// The intermediate progress of state root computation.
    /// Contains the walker stack, the hash builder and the trie updates.
    Progress(Box<IntermediateStateRootState>, usize, TrieUpdates),
}

/// The intermediate state of the state root computation.
#[derive(Debug)]
pub struct IntermediateStateRootState {
    /// Previously constructed hash builder.
    pub hash_builder: HashBuilder,
    /// Previously recorded walker stack.
    pub walker_stack: Vec<CursorSubNode>,
    /// The last hashed account key processed.
    pub last_account_key: B256,
}

impl From<MerkleCheckpoint> for IntermediateStateRootState {
    fn from(value: MerkleCheckpoint) -> Self {
        Self {
            hash_builder: HashBuilder::from(value.state),
            walker_stack: value.walker_stack.into_iter().map(CursorSubNode::from).collect(),
            last_account_key: value.last_account_key,
        }
    }
}
