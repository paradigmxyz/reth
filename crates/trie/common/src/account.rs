use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::B256;
use alloy_trie::TrieAccount;
use reth_primitives_traits::Account;
use revm_primitives::AccountInfo;

/// Convert an `Account` to a `TrieAccount` of alloy
pub fn from_account_to_trie_account(account: Account, storage_root: B256) -> TrieAccount {
    TrieAccount {
        nonce: account.nonce,
        balance: account.balance,
        storage_root,
        code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
    }
}

/// Convert an `AccountInfo` to a `TrieAccount` of alloy
pub fn from_account_info_to_trie_account(account: AccountInfo, storage_root: B256) -> TrieAccount {
    TrieAccount {
        nonce: account.nonce,
        balance: account.balance,
        storage_root,
        code_hash: account.code_hash,
    }
}
