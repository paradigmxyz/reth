use crate::{
    constants::EMPTY_ROOT_HASH, proofs, Account, GenesisAccount, B256, KECCAK_EMPTY, U256,
};
use alloy_primitives::keccak256;
use alloy_rlp::{RlpDecodable, RlpEncodable};
use revm_primitives::AccountInfo;

/// An Ethereum account as represented in the trie.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
pub struct TrieAccount {
    /// Account nonce.
    nonce: u64,
    /// Account balance.
    balance: U256,
    /// Account's storage root.
    storage_root: B256,
    /// Hash of the account's bytecode.
    code_hash: B256,
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

impl From<GenesisAccount> for TrieAccount {
    fn from(account: GenesisAccount) -> Self {
        let storage_root = account
            .storage
            .map(|storage| {
                proofs::storage_root_unhashed(
                    storage
                        .into_iter()
                        .filter(|(_, value)| *value != B256::ZERO)
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

impl TrieAccount {
    /// Get account's storage root.
    pub fn storage_root(&self) -> B256 {
        self.storage_root
    }
}
