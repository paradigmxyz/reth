/// Re-export for convenience.
pub use alloy_trie::TrieAccount;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::storage_root_unhashed;
    use alloy_consensus::constants::KECCAK_EMPTY;
    use alloy_genesis::GenesisAccount;
    use alloy_primitives::{keccak256, Bytes, B256, U256};
    use std::collections::BTreeMap;

    use alloy_trie::EMPTY_ROOT_HASH;
    use reth_primitives_traits::Account;

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

        // Check that the default Account converts to the same TrieAccount
        assert_eq!(Account::default().into_trie_account(EMPTY_ROOT_HASH), trie_account);
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

        // Check that the Account converts to the same TrieAccount
        assert_eq!(
            Account {
                nonce: 10,
                balance: U256::from(1000),
                bytecode_hash: Some(keccak256([0x60, 0x61]))
            }
            .into_trie_account(expected_storage_root),
            trie_account
        );
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
