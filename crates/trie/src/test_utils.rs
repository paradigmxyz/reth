use alloy_rlp::encode_fixed_size;
use reth_primitives::{
    proofs::triehash::KeccakHasher, trie::TrieAccount, Account, Address, B256, U256,
};

/// Re-export of [triehash].
pub use triehash;

/// Compute the state root of a given set of accounts using [triehash::sec_trie_root].
pub fn state_root<I, S>(accounts: I) -> B256
where
    I: IntoIterator<Item = (Address, (Account, S))>,
    S: IntoIterator<Item = (B256, U256)>,
{
    let encoded_accounts = accounts.into_iter().map(|(address, (account, storage))| {
        let storage_root = storage_root(storage);
        let account = TrieAccount::from((account, storage_root));
        (address, alloy_rlp::encode(account))
    });
    triehash::sec_trie_root::<KeccakHasher, _, _, _>(encoded_accounts)
}

/// Compute the storage root for a given account using [triehash::sec_trie_root].
pub fn storage_root<I: IntoIterator<Item = (B256, U256)>>(storage: I) -> B256 {
    let encoded_storage = storage.into_iter().map(|(k, v)| (k, encode_fixed_size(&v)));
    triehash::sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage)
}

/// Compute the state root of a given set of accounts with prehashed keys using
/// [triehash::trie_root].
pub fn state_root_prehashed<I, S>(accounts: I) -> B256
where
    I: IntoIterator<Item = (B256, (Account, S))>,
    S: IntoIterator<Item = (B256, U256)>,
{
    let encoded_accounts = accounts.into_iter().map(|(address, (account, storage))| {
        let storage_root = storage_root_prehashed(storage);
        let account = TrieAccount::from((account, storage_root));
        (address, alloy_rlp::encode(account))
    });

    triehash::trie_root::<KeccakHasher, _, _, _>(encoded_accounts)
}

/// Compute the storage root for a given account with prehashed slots using [triehash::trie_root].
pub fn storage_root_prehashed<I: IntoIterator<Item = (B256, U256)>>(storage: I) -> B256 {
    let encoded_storage = storage.into_iter().map(|(k, v)| (k, encode_fixed_size(&v)));
    triehash::trie_root::<KeccakHasher, _, _, _>(encoded_storage)
}
