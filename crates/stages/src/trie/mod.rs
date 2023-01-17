#![allow(missing_docs, dead_code, unused_variables, unused_imports)]
use std::{collections::HashMap, marker::PhantomData};

use crate::Transaction;
use bytes::BytesMut;
use hash256_std_hasher::Hash256StdHasher;
use hash_db::{AsHashDB, Prefix};
use memory_db::{HashKey, MemoryDB};
use reference_trie::ReferenceNodeCodec;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    models::AccountBeforeTx,
    table::{Decode, Encode, Table},
    tables,
    transaction::DbTx,
};
use reth_primitives::{
    keccak256, proofs::KeccakHasher, rpc::H160, Account, Address, StorageEntry, H256, KECCAK_EMPTY,
    U256,
};
use reth_rlp::{Encodable, RlpDecodable, RlpEncodable};
use trie_db::{
    CError, HashDB, Hasher, NodeCodec, TrieDBMut, TrieDBMutBuilder, TrieLayout, TrieMut,
};

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

impl From<Account> for EthAccount {
    fn from(acc: Account) -> Self {
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
    pub(crate) fn calculate_root<DB: Database>(&mut self, tx: &Transaction<'_, DB>) -> H256 {
        let mut accounts_cursor = tx.cursor_read::<tables::PlainAccountState>().unwrap();
        let mut walker = accounts_cursor.walk(Address::zero()).unwrap();
        // let trie_cursor = tx.cursor_read::<tables::AccountsTrie>().unwrap();

        let mut db = MemoryDB::<KeccakHasher, HashKey<KeccakHasher>, Vec<u8>>::default();
        let mut root = H256::zero();
        let mut trie: TrieDBMut<'_, DBTrieLayout> =
            TrieDBMutBuilder::new(&mut db, &mut root).build();

        while let Some((address, account)) = walker.next().transpose().unwrap() {
            let mut key = EthAccount::from(account);

            // storage_root
            key.storage_root = self.calculate_storage_root(tx, address);

            let mut bytes = BytesMut::new();
            Encodable::encode(&key, &mut bytes);
            trie.insert(address.as_bytes(), &bytes).unwrap();
        }

        *trie.root()
    }

    // Result<H256>
    fn calculate_storage_root<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
        address: Address,
    ) -> H256 {
        let mut db = MemoryDB::<KeccakHasher, HashKey<KeccakHasher>, Vec<u8>>::default();
        let mut root = H256::zero();
        let mut trie: TrieDBMut<'_, DBTrieLayout> =
            TrieDBMutBuilder::new(&mut db, &mut root).build();

        let mut storage_cursor = tx.cursor_dup_read::<tables::PlainStorageState>().unwrap();
        let mut walker = storage_cursor.walk_dup(address, H256::zero()).unwrap();

        while let Some((_, StorageEntry { key: storage_key, value })) =
            walker.next().transpose().unwrap()
        {
            let mut bytes = BytesMut::new();
            let location = [H256::from(address).as_bytes(), storage_key.as_bytes()].concat();
            Encodable::encode(&value, &mut bytes);
            trie.insert(location.as_slice(), &bytes).unwrap();
        }

        *trie.root()
    }
}

pub(crate) fn gather_account_changes() {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db::{
        mdbx::{test_utils::create_test_rw_db, WriteMap},
        tables,
    };
    use reth_primitives::{hex_literal::hex, proofs::EMPTY_ROOT, Address, KECCAK_EMPTY};
    use std::str::FromStr;
    use trie_db::TrieDBMutBuilder;

    #[test]
    fn empty_trie() {
        let mut trie = DBTrieLoader {};
        let db = create_test_rw_db::<WriteMap>();
        let tx = Transaction::new(db.as_ref()).unwrap();
        assert_eq!(trie.calculate_root(&tx), EMPTY_ROOT);
    }
}
