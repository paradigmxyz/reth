use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;

/// A Scroll account as represented in the trie.
#[derive(Debug)]
pub struct ScrollTrieAccount {
    /// nonce
    pub nonce: u64,
    /// code size
    pub code_size: u64,
    /// balance
    pub balance: U256,
    /// storage root
    pub storage_root: B256,
    /// keccak code hash
    pub code_hash: B256,
    /// poseidon code hash
    pub poseidon_code_hash: B256,
}

impl From<(Account, B256)> for ScrollTrieAccount {
    fn from((account, storage_root): (Account, B256)) -> Self {
        Self {
            nonce: account.nonce,
            balance: account.balance,
            storage_root,
            code_hash: account.get_bytecode_hash(),
            #[cfg(feature = "scroll")]
            poseidon_code_hash: account.get_poseidon_code_hash(),
            #[cfg(feature = "scroll")]
            code_size: account.get_code_size(),
            #[cfg(not(feature = "scroll"))]
            poseidon_code_hash: B256::default(),
            #[cfg(not(feature = "scroll"))]
            code_size: 0,
        }
    }
}
