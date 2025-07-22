use crate::{hash_builder::HashBuilder, trie_cursor::CursorSubNode, updates::TrieUpdates};
use alloy_primitives::B256;
use reth_stages_types::MerkleCheckpoint;

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
    /// The intermediate account root state.
    pub account_root_state: IntermediateRootState,
    /// The intermediate storage root state.
    pub storage_root_state: Option<IntermediateRootState>,
}

impl From<MerkleCheckpoint> for IntermediateStateRootState {
    fn from(value: MerkleCheckpoint) -> Self {
        Self {
            account_root_state: IntermediateRootState {
                hash_builder: HashBuilder::from(value.state),
                walker_stack: value.walker_stack.into_iter().map(CursorSubNode::from).collect(),
                last_account_key: value.last_account_key,
            },
            // TODO: update with intermediate storage state from checkpoint
            storage_root_state: None,
        }
    }
}

/// The intermediate state of a state root computation, whether account or storage root.
#[derive(Debug)]
pub struct IntermediateRootState {
    /// Previously constructed hash builder.
    pub hash_builder: HashBuilder,
    /// Previously recorded walker stack.
    pub walker_stack: Vec<CursorSubNode>,
    /// The last hashed account key processed.
    pub last_account_key: B256,
}
