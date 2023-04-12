use crate::account::EthAccount;
use reth_primitives::{proofs::KeccakHasher, Account, Address, H256, U256};
use reth_rlp::{encode_fixed_size, Encodable};

/// Re-export of [triehash].
pub use triehash;

/// Compute the state root of a given set of accounts using [triehash::sec_trie_root].
pub fn state_root<I, S>(accounts: I) -> H256
where
    I: Iterator<Item = (Address, (Account, S))>,
    S: IntoIterator<Item = (H256, U256)>,
{
    let encoded_accounts = accounts.map(|(address, (account, storage))| {
        let storage_root = storage_root(storage.into_iter());
        let mut out = Vec::new();
        EthAccount::from(account).with_storage_root(storage_root).encode(&mut out);
        (address, out)
    });

    triehash::sec_trie_root::<KeccakHasher, _, _, _>(encoded_accounts)
}

/// Compute the storage root for a given account using [triehash::sec_trie_root].
pub fn storage_root<I: Iterator<Item = (H256, U256)>>(storage: I) -> H256 {
    let encoded_storage = storage.map(|(k, v)| (k, encode_fixed_size(&v).to_vec()));
    triehash::sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage)
}

/// Compute the state root of a given set of accounts with prehashed keys using
/// [triehash::trie_root].
pub fn state_root_prehashed<I, S>(accounts: I) -> H256
where
    I: Iterator<Item = (H256, (Account, S))>,
    S: IntoIterator<Item = (H256, U256)>,
{
    let encoded_accounts = accounts.map(|(address, (account, storage))| {
        let storage_root = storage_root_prehashed(storage.into_iter());
        let mut out = Vec::new();
        EthAccount::from(account).with_storage_root(storage_root).encode(&mut out);
        (address, out)
    });

    triehash::trie_root::<KeccakHasher, _, _, _>(encoded_accounts)
}

/// Compute the storage root for a given account with prehashed slots using [triehash::trie_root].
pub fn storage_root_prehashed<I: Iterator<Item = (H256, U256)>>(storage: I) -> H256 {
    let encoded_storage = storage.map(|(k, v)| (k, encode_fixed_size(&v).to_vec()));
    triehash::trie_root::<KeccakHasher, _, _, _>(encoded_storage)
}
