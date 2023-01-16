#![allow(missing_docs, dead_code, unused_variables, unused_imports)]
use std::{collections::HashMap, marker::PhantomData};

use bytes::BytesMut;
use hash256_std_hasher::Hash256StdHasher;
use hash_db::{AsHashDB, Prefix};
use memory_db::{HashKey, MemoryDB};
use reference_trie::ReferenceNodeCodec;
use reth_db::{
    database::Database,
    models::AccountBeforeTx,
    table::{Decode, Encode, Table},
};
use reth_primitives::{keccak256, rpc::H160, Account, Address, H256, KECCAK_EMPTY, U256};
use reth_rlp::{Encodable, RlpDecodable, RlpEncodable};
use trie_db::{
    CError, HashDB, Hasher, NodeCodec, TrieDBMut, TrieDBMutBuilder, TrieLayout, TrieMut,
};

// pub struct DBTrie<'this, DB, T>
// where
//     DB: Database,
//     T: Table,
//     T::Key: Encode + Decode,
// {
//     trie: TrieDBMut<'this, DBTrieLayout>,
//     _t: PhantomData<(DB, T)>,
// }

// impl<'this, 'db, DB: Database + 'this, T: Table> DBTrie<'this, DB, T>
// where
//     DB: Database,
//     T: Table,
//     T::Key: Encode + Decode,
//     T::Value: From<Vec<u8>>,
// {
//     pub fn new(hash_db: &'this mut HashDatabase<DB>, root: &'this mut H256) -> Self {
//         let builder = TrieDBMutBuilder::new(hash_db, root);
//         Self { trie: builder.build(), _t: Default::default() }
//     }

//     pub fn get(self, key: T::Key) -> Result<Option<T::Value>, TrieError> {
//         let value = self.trie.get(key.encode().as_ref())?;
//         Ok(value.map(|v| T::Value::from(v)))
//     }
// }

// #[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
// pub enum TrieError {
//     #[error("{0:?}")]
//     ImplError(#[from] Box<trie_db::TrieError<reth_primitives::H256, parity_scale_codec::Error>>),
//     #[error("{0:?}")]
//     DecodeError(#[from] reth_db::Error),
// }

struct DBTrieLayout;

impl TrieLayout for DBTrieLayout {
    const USE_EXTENSION: bool = true;

    // TODO: modify?
    const ALLOW_EMPTY: bool = false;
    // I think non-inlined nodes aren't supported
    const MAX_INLINE_VALUE: Option<u32> = None;

    type Hash = KeccakHasher;
    type Codec = ReferenceNodeCodec<Self::Hash>;
}

struct KeccakHasher;

impl Hasher for KeccakHasher {
    type Out = H256;

    type StdHasher = Hash256StdHasher;

    const LENGTH: usize = 256 / 8;

    fn hash(x: &[u8]) -> Self::Out {
        keccak256(x)
    }
}

// pub struct HashDatabase<DB: Database> {
//     db: Arc<DB>,
// }

// impl<H: Hasher, DB: Database, T> HashDB<H, T> for HashDatabase<DB> {
//     fn get(&self, key: &H::Out, prefix: Prefix<'_>) -> Option<T> {
//         todo!()
//     }

//     fn contains(&self, key: &H::Out, prefix: Prefix<'_>) -> bool {
//         todo!()
//     }

//     fn insert(&mut self, prefix: Prefix<'_>, value: &[u8]) -> H::Out {
//         todo!()
//     }

//     fn emplace(&mut self, key: H::Out, prefix: Prefix<'_>, value: T) {
//         todo!()
//     }

//     fn remove(&mut self, key: &H::Out, prefix: Prefix<'_>) {
//         todo!()
//     }
// }

// impl<H: Hasher, T, DB: Database> AsHashDB<H, T> for HashDatabase<DB> {
//     fn as_hash_db(&self) -> &dyn HashDB<H, T> {
//         self
//     }

//     fn as_hash_db_mut<'a>(&'a mut self) -> &'a mut (dyn HashDB<H, T> + 'a) {
//         self
//     }
// }

/// An Ethereum account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
struct EthAccount {
    /// Account nonce.
    nonce: u64,
    /// Account balance.
    balance: U256,
    /// Account's storage root.
    storage_root: H256,
    /// Hash of the account's bytecode.
    code_hash: H256,
}

impl From<&Account> for EthAccount {
    fn from(acc: &Account) -> Self {
        EthAccount {
            nonce: acc.nonce,
            balance: acc.balance,
            storage_root: H256::zero(),
            code_hash: acc.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        }
    }
}

pub(crate) struct DBTrieLoader;

impl DBTrieLoader {
    // Result<H256>
    pub(crate) fn calculate_root(&mut self) -> H256 {
        let account_changes = vec![AccountBeforeTx { address: Address::zero(), info: None }];
        let accounts = HashMap::from([(
            Address::zero(),
            Account { nonce: 0, balance: U256::from(100), bytecode_hash: None },
        )]);
        let mut db = MemoryDB::<KeccakHasher, HashKey<KeccakHasher>, Vec<u8>>::default();
        let mut root = H256::zero();
        let mut trie: TrieDBMut<'_, DBTrieLayout> =
            TrieDBMutBuilder::new(&mut db, &mut root).build();

        for AccountBeforeTx { address, info } in account_changes {
            let Account { nonce, balance, bytecode_hash } = accounts.get(&address).unwrap();
            let mut account = EthAccount::from(accounts.get(&address).unwrap());

            // storage_root
            account.storage_root = self.calculate_storage_root(&address);

            let mut bytes = BytesMut::new();
            account.encode(&mut bytes);
            trie.insert(address.as_bytes(), &bytes).unwrap();
        }
        *trie.root()
    }

    // Result<H256>
    fn calculate_storage_root(&mut self, address: &Address) -> H256 {
        H256::zero()
    }
}

pub(crate) fn gather_account_changes() {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::tables;
    use reth_primitives::{hex_literal::hex, Address, KECCAK_EMPTY};
    use std::str::FromStr;
    use trie_db::TrieDBMutBuilder;

    #[test]
    fn empty_trie() {
        let mut trie = DBTrieLoader {};
        // assert_eq!(
        //     trie.calculate_root(),
        //     H256::from_str("bc36789e7a1e281436464229828f817d6612f7b477d66591ff96a9e064bcc98a")
        //         .unwrap()
        // );
    }
}
