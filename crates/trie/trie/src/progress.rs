use crate::{
    hash_builder::HashBuilder,
    trie_cursor::CursorSubNode,
    updates::{StorageTrieUpdates, TrieUpdates},
};
use alloy_primitives::B256;
use reth_primitives_traits::Account;
use reth_stages_types::MerkleCheckpoint;

/// The progress of the state root computation.
#[derive(Debug)]
pub enum StateRootProgress {
    /// The complete state root computation with updates, the total number of entries walked, and
    /// the computed root.
    Complete(B256, usize, TrieUpdates),
    /// The intermediate progress of state root computation.
    /// Contains the walker stack, the hash builder, and the trie updates.
    ///
    /// Also contains any progress in an inner storage root computation.
    Progress(Box<IntermediateStateRootState>, usize, TrieUpdates),
}

/// The intermediate state of the state root computation.
#[derive(Debug)]
pub struct IntermediateStateRootState {
    /// The intermediate account root state.
    pub account_root_state: IntermediateRootState,
    /// The intermediate storage root state with account data.
    pub storage_root_state: Option<IntermediateStorageRootState>,
}

/// The intermediate state of a storage root computation along with the account.
#[derive(Debug)]
pub struct IntermediateStorageRootState {
    /// The intermediate storage trie state.
    pub state: IntermediateRootState,
    /// The account for which the storage root is being computed.
    pub account: Account,
}

impl From<MerkleCheckpoint> for IntermediateStateRootState {
    fn from(value: MerkleCheckpoint) -> Self {
        Self {
            account_root_state: IntermediateRootState {
                hash_builder: HashBuilder::from(value.state),
                walker_stack: value.walker_stack.into_iter().map(CursorSubNode::from).collect(),
                last_hashed_key: value.last_account_key,
            },
            storage_root_state: value.storage_root_checkpoint.map(|checkpoint| {
                IntermediateStorageRootState {
                    state: IntermediateRootState {
                        hash_builder: HashBuilder::from(checkpoint.state),
                        walker_stack: checkpoint
                            .walker_stack
                            .into_iter()
                            .map(CursorSubNode::from)
                            .collect(),
                        last_hashed_key: checkpoint.last_storage_key,
                    },
                    account: Account {
                        nonce: checkpoint.account_nonce,
                        balance: checkpoint.account_balance,
                        bytecode_hash: Some(checkpoint.account_bytecode_hash),
                    },
                }
            }),
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
    /// The last hashed key processed.
    pub last_hashed_key: B256,
}

/// The progress of a storage root calculation.
#[derive(Debug)]
pub enum StorageRootProgress {
    /// The complete storage root computation with updates and computed root.
    Complete(B256, usize, StorageTrieUpdates),
    /// The intermediate progress of state root computation.
    /// Contains the walker stack, the hash builder, and the trie updates.
    Progress(Box<IntermediateRootState>, usize, StorageTrieUpdates),
}
