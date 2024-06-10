use crate::Account;
use alloy_consensus::constants::{EMPTY_ROOT_HASH, KECCAK_EMPTY};
use alloy_genesis::GenesisAccount;
use alloy_primitives::{keccak256, B256, U256};
use reth_trie_types::TrieAccount;
use revm_primitives::AccountInfo;

/// Converts a type into a [`TrieAccount`].
pub trait IntoTrieAccount {
    /// Converts to this type into a [`TrieAccount`].
    fn to_trie_account(self) -> TrieAccount;
}

impl IntoTrieAccount for GenesisAccount {
    fn to_trie_account(self) -> TrieAccount {
        let storage_root = self
            .storage
            .map(|storage| {
                super::storage_root_unhashed(
                    storage
                        .into_iter()
                        .filter(|(_, value)| *value != B256::ZERO)
                        .map(|(slot, value)| (slot, U256::from_be_bytes(*value))),
                )
            })
            .unwrap_or(EMPTY_ROOT_HASH);

        TrieAccount::new(
            self.nonce.unwrap_or_default(),
            self.balance,
            storage_root,
            self.code.map_or(KECCAK_EMPTY, keccak256),
        )
    }
}

impl IntoTrieAccount for (Account, B256) {
    fn to_trie_account(self) -> TrieAccount {
        let (account, storage_root) = self;
        TrieAccount::new(
            account.nonce,
            account.balance,
            storage_root,
            account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        )
    }
}

impl IntoTrieAccount for (AccountInfo, B256) {
    fn to_trie_account(self) -> TrieAccount {
        let (account, storage_root) = self;
        TrieAccount::new(account.nonce, account.balance, storage_root, account.code_hash)
    }
}
