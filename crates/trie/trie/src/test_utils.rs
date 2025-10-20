use alloy_primitives::{Address, B256, U256};
use reth_primitives_traits::Account;

// Re-export alloy-trie functions
pub use alloy_trie::root::{
    storage_root, storage_root_unhashed, storage_root_unhashed as storage_root_prehashed,
};

/// Compute the state root of a given set of accounts using
/// [`alloy_trie::root::state_root_unhashed`].
pub fn state_root<I, S>(accounts: I) -> B256
where
    I: IntoIterator<Item = (Address, (Account, S))>,
    S: IntoIterator<Item = (B256, U256)>,
{
    let accounts = accounts.into_iter().map(|(address, (account, storage))| {
        let storage_root = storage_root(storage);
        let account = account.into_trie_account(storage_root);
        (address, account)
    });
    alloy_trie::root::state_root_unhashed(accounts)
}

/// Compute the state root of a given set of accounts with prehashed keys using
/// [`alloy_trie::root::state_root`].
pub fn state_root_prehashed<I, S>(accounts: I) -> B256
where
    I: IntoIterator<Item = (B256, (Account, S))>,
    S: IntoIterator<Item = (B256, U256)>,
{
    let accounts = accounts.into_iter().map(|(address, (account, storage))| {
        let storage_root = storage_root_unhashed(storage);
        let account = account.into_trie_account(storage_root);
        (address, account)
    });

    alloy_trie::root::state_root(accounts)
}
