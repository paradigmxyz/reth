use alloy_primitives::{B256, U256};
use alloy_rlp::{RlpDecodable, RlpEncodable};

/// An Ethereum account as represented in the trie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
pub struct TrieAccount {
    /// Account nonce.
    pub nonce: u64,
    /// Account balance.
    pub balance: U256,
    /// Account's storage root.
    pub storage_root: B256,
    /// Hash of the account's bytecode.
    pub code_hash: B256,
}

impl TrieAccount {
    /// Get account's storage root.
    pub const fn storage_root(&self) -> B256 {
        self.storage_root
    }
}
