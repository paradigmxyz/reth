use crate::{Address, H256};
use reth_codecs::{main_codec, Compact};

/// Saves the progress of MerkleStage
#[main_codec]
#[derive(Default, Debug, Copy, Clone, PartialEq)]
pub struct ProofCheckpoint {
    /// The next hashed account to insert into the trie.
    pub hashed_address: Option<H256>,
    /// The next storage entry to insert into the trie.
    pub storage_key: Option<H256>,
    /// Current intermediate root for `AccountsTrie`.
    pub account_root: Option<H256>,
    /// Current intermediate storage root from an account.
    pub storage_root: Option<H256>,
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
