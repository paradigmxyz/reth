use crate::{
    trie::{HashBuilderState, StoredSubNode},
    Address, H256,
};
use reth_codecs::{main_codec, Compact};

/// Saves the progress of Merkle stage.
#[main_codec]
#[derive(Default, Debug, Clone, PartialEq)]
pub struct MerkleCheckpoint {
    // TODO: target block?
    /// The last hashed account key processed.
    pub last_account_key: H256,
    /// The last walker key processed.
    pub last_walker_key: Vec<u8>,
    /// Previously recorded walker stack.
    pub walker_stack: Vec<StoredSubNode>,
    /// The hash builder state.
    pub state: HashBuilderState,
}

/// Saves the progress of AccountHashing
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct AccountHashingCheckpoint {
    /// The next account to start hashing from
    pub address: Option<Address>,
    /// Start transition id
    pub from: u64,
    /// Last transition id
    pub to: u64,
}

/// Saves the progress of StorageHashing
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct StorageHashingCheckpoint {
    /// The next account to start hashing from
    pub address: Option<Address>,
    /// The next storage slot to start hashing from
    pub storage: Option<H256>,
    /// Start transition id
    pub from: u64,
    /// Last transition id
    pub to: u64,
}
