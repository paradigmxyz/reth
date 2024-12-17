use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_genesis::GenesisAccount;
use alloy_primitives::{keccak256, B256, U256};
use reth_primitives_traits::Account;
use reth_scroll_primitives::poseidon::POSEIDON_EMPTY;

/// A Scroll account as represented in the trie.
#[derive(Debug, Clone)]
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

impl From<GenesisAccount> for ScrollTrieAccount {
    fn from(account: GenesisAccount) -> Self {
        let storage_root = account
            .storage
            .map(|storage| {
                crate::storage_root_unhashed(
                    storage
                        .into_iter()
                        .filter(|(_, value)| !value.is_zero())
                        .map(|(slot, value)| (slot, U256::from_be_bytes(*value))),
                )
            })
            .unwrap_or(reth_scroll_primitives::poseidon::EMPTY_ROOT_HASH);

        Self {
            nonce: account.nonce.unwrap_or_default(),
            balance: account.balance,
            storage_root,
            code_hash: account.code.as_ref().map_or(KECCAK_EMPTY, keccak256),
            poseidon_code_hash: account
                .code
                .as_ref()
                .map_or(POSEIDON_EMPTY, reth_scroll_primitives::poseidon::hash_code),
            code_size: account.code.map(|c| c.len()).unwrap_or_default() as u64,
        }
    }
}
