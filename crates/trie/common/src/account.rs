use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::B256;
use alloy_trie::TrieAccount;
use reth_primitives_traits::Account;

/// Wrapper type to implement foreign trait conversions
#[derive(Debug)]
pub struct AccountWithStorageRoot(pub Account, pub B256);

impl From<AccountWithStorageRoot> for TrieAccount {
    fn from(AccountWithStorageRoot(account, storage_root): AccountWithStorageRoot) -> Self {
        Self {
            nonce: account.nonce,
            balance: account.balance,
            storage_root,
            code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::storage_root_unhashed;
    use alloy_genesis::GenesisAccount;
    use alloy_primitives::{keccak256, Bytes, U256};
    use alloy_trie::{TrieAccount, EMPTY_ROOT_HASH};
    use std::collections::BTreeMap;

    #[test]
    fn test_from_genesis_account_with_default_values() {
        let genesis_account = GenesisAccount::default();

        // Convert the GenesisAccount to a TrieAccount
        let trie_account: TrieAccount = genesis_account.into();

        // Check the fields are properly set.
        assert_eq!(trie_account.nonce, 0);
        assert_eq!(trie_account.balance, U256::default());
        assert_eq!(trie_account.storage_root, EMPTY_ROOT_HASH);
        assert_eq!(trie_account.code_hash, KECCAK_EMPTY);
    }

    #[test]
    fn test_from_genesis_account_with_values() {
        // Create a GenesisAccount with specific values
        let mut storage = BTreeMap::new();
        storage.insert(B256::from([0x01; 32]), B256::from([0x02; 32]));

        let genesis_account = GenesisAccount {
            nonce: Some(10),
            balance: U256::from(1000),
            code: Some(Bytes::from(vec![0x60, 0x61])),
            storage: Some(storage),
            private_key: None,
        };

        // Convert the GenesisAccount to a TrieAccount
        let trie_account: TrieAccount = genesis_account.into();

        let expected_storage_root = storage_root_unhashed(BTreeMap::from([(
            B256::from([0x01; 32]),
            U256::from_be_bytes(*B256::from([0x02; 32])),
        )]));

        // Check that the fields are properly set.
        assert_eq!(trie_account.nonce, 10);
        assert_eq!(trie_account.balance, U256::from(1000));
        assert_eq!(trie_account.storage_root, expected_storage_root);
        assert_eq!(trie_account.code_hash, keccak256([0x60, 0x61]));
    }

    #[test]
    fn test_from_genesis_account_with_zeroed_storage_values() {
        // Create a GenesisAccount with storage containing zero values
        let storage = BTreeMap::from([(B256::from([0x01; 32]), B256::from([0x00; 32]))]);

        let genesis_account = GenesisAccount {
            nonce: Some(3),
            balance: U256::from(300),
            code: None,
            storage: Some(storage),
            private_key: None,
        };

        // Convert the GenesisAccount to a TrieAccount
        let trie_account: TrieAccount = genesis_account.into();

        // Check the fields are properly set.
        assert_eq!(trie_account.nonce, 3);
        assert_eq!(trie_account.balance, U256::from(300));
        // Zero values in storage should result in EMPTY_ROOT_HASH
        assert_eq!(trie_account.storage_root, EMPTY_ROOT_HASH);
        // No code provided, so code hash should be KECCAK_EMPTY
        assert_eq!(trie_account.code_hash, KECCAK_EMPTY);
    }
}
