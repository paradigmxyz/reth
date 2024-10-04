use crate::root::storage_root_unhashed;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_genesis::GenesisAccount;
use alloy_primitives::{keccak256, B256, U256};
use alloy_rlp::{RlpDecodable, RlpEncodable};
use alloy_trie::EMPTY_ROOT_HASH;
use reth_primitives_traits::Account;
use revm_primitives::AccountInfo;

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

impl From<GenesisAccount> for TrieAccount {
    fn from(account: GenesisAccount) -> Self {
        let storage_root = account
            .storage
            .map(|storage| {
                storage_root_unhashed(
                    storage
                        .into_iter()
                        .filter(|(_, value)| !value.is_zero())
                        .map(|(slot, value)| (slot, U256::from_be_bytes(*value))),
                )
            })
            .unwrap_or(EMPTY_ROOT_HASH);

        Self {
            nonce: account.nonce.unwrap_or_default(),
            balance: account.balance,
            storage_root,
            code_hash: account.code.map_or(KECCAK_EMPTY, keccak256),
        }
    }
}

impl From<(Account, B256)> for TrieAccount {
    fn from((account, storage_root): (Account, B256)) -> Self {
        Self {
            nonce: account.nonce,
            balance: account.balance,
            storage_root,
            code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        }
    }
}

impl From<(AccountInfo, B256)> for TrieAccount {
    fn from((account, storage_root): (AccountInfo, B256)) -> Self {
        Self {
            nonce: account.nonce,
            balance: account.balance,
            storage_root,
            code_hash: account.code_hash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;
    use std::collections::BTreeMap;

    #[test]
    fn test_from_genesis_account_with_default_values() {
        let genesis_account = GenesisAccount::default();

        // Convert the GenesisAccount to a TrieAccount
        let trie_account: TrieAccount = genesis_account.into();

        // Check the fields are properly set.
        assert_eq!(trie_account.nonce, 0);
        assert_eq!(trie_account.balance, U256::default());
        assert_eq!(trie_account.storage_root(), EMPTY_ROOT_HASH);
        assert_eq!(trie_account.code_hash, KECCAK_EMPTY);

        // Check that the default Account converts to the same TrieAccount
        assert_eq!(Into::<TrieAccount>::into((Account::default(), EMPTY_ROOT_HASH)), trie_account);

        // Check that the default AccountInfo converts to the same TrieAccount
        assert_eq!(
            Into::<TrieAccount>::into((AccountInfo::default(), EMPTY_ROOT_HASH)),
            trie_account
        );
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
        assert_eq!(trie_account.storage_root(), expected_storage_root);
        assert_eq!(trie_account.code_hash, keccak256([0x60, 0x61]));

        // Check that the Account converts to the same TrieAccount
        assert_eq!(
            Into::<TrieAccount>::into((
                Account {
                    nonce: 10,
                    balance: U256::from(1000),
                    bytecode_hash: Some(keccak256([0x60, 0x61]))
                },
                expected_storage_root
            )),
            trie_account
        );

        // Check that the AccountInfo converts to the same TrieAccount
        assert_eq!(
            Into::<TrieAccount>::into((
                AccountInfo {
                    nonce: 10,
                    balance: U256::from(1000),
                    code_hash: keccak256([0x60, 0x61]),
                    ..Default::default()
                },
                expected_storage_root
            )),
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
        assert_eq!(trie_account.storage_root(), EMPTY_ROOT_HASH);
        // No code provided, so code hash should be KECCAK_EMPTY
        assert_eq!(trie_account.code_hash, KECCAK_EMPTY);
    }
}
