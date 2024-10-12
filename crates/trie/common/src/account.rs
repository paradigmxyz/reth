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
