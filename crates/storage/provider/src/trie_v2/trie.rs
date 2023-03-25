// 1. Calculate Root given entire presorted hashed thing
// Must work with:
// 1. Storage Trie Cursor
// 2. Accounts Trie Cursor
// 2. Update root given a list of updates
// Be able to calculate incremental state root without taking a write lock

use super::{
    nibbles::Nibbles,
    node::{empty_children, BranchNode, ExtensionNode, HashNode, LeafNode, Node},
};

use reth_db::{
    cursor::DbDupCursorRO,
    tables,
    transaction::{DbTx, DbTxMut},
    Error as DbError,
};
use reth_primitives::{keccak256, Address, StorageEntry, H256};
use std::error::Error;
use thiserror::Error;

// Create a walker at a specific prefix on the database.
// Pass a cursor to the hashed storage table (or hashed account later)
// For each element:
// 1. If its value is 0, delete it from the Hashed table
// 2. Nibble(key) & Some(rlp(value)) or None
pub struct StorageRoot<TX> {
    pub tx: TX,
    pub address: Address,
}

impl<'a, TX: DbTx<'a>> StorageRoot<TX> {
    /// Creates a new storage root calculator
    pub fn new(tx: TX, address: Address) -> Self {
        Self { tx, address }
    }
}

#[derive(Error, Debug)]
pub enum StorageRootError {
    #[error(transparent)]
    DB(#[from] DbError),
}

impl<'a, TX: DbTx<'a>> StorageRoot<TX> {
    /// Walks the entire hashed storage table entry for the given address and calculates the storage
    /// root
    pub fn root(&self) -> Result<H256, StorageRootError> {
        // Instantiate the walker
        let mut cursor = self.tx.cursor_dup_read::<tables::HashedStorage>()?;
        let hashed_address = keccak256(self.address);
        dbg!(&cursor.next_dup_val());
        let mut walker = cursor.walk_dup(Some(hashed_address), None)?;

        dbg!(&hashed_address);
        while let Some(item) = walker.next() {
            let (hashed_address, entry) = item?;
            let StorageEntry { key: hashed_slot, value } = entry;
            dbg!(&hashed_address, &hashed_slot, &value);
        }

        Ok(unimplemented!())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::Transaction;
    use std::ops::DerefMut;

    use super::*;
    use assert_matches::assert_matches;
    use proptest::{prelude::ProptestConfig, proptest};
    use reth_db::{
        database::{Database, DatabaseGAT},
        mdbx::{test_utils::create_test_rw_db, Env, WriteMap},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives::{
        hex_literal::hex,
        keccak256,
        proofs::{genesis_state_root, KeccakHasher, EMPTY_ROOT},
        Address, Bytes, ChainSpec, Genesis, MAINNET,
    };
    use reth_rlp::encode_fixed_size;
    use std::{collections::HashMap, ops::Deref, str::FromStr};
    use triehash::sec_trie_root;

    use reth_primitives::{Account, H256, U256};

    fn insert_account<'a, TX: DbTxMut<'a>>(
        tx: &mut TX,
        address: Address,
        account: Account,
        storage: &HashMap<H256, U256>,
    ) {
        let hashed_address = keccak256(address);
        dbg!(&hashed_address);
        tx.put::<tables::HashedAccount>(hashed_address, account).unwrap();

        for (k, v) in storage {
            tx.put::<tables::HashedStorage>(
                hashed_address,
                StorageEntry { key: keccak256(k), value: *v },
            )
            .unwrap();
        }
    }

    fn storage_root(storage: &HashMap<H256, U256>) -> H256 {
        let encoded_storage = storage.iter().map(|(k, v)| {
            let out = encode_fixed_size(v).to_vec();
            (k, out)
        });

        H256(sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage).0)
    }

    #[test]
    fn storage_root_full() {
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();

        let address = Address::random();
        let storage = HashMap::from([
            (H256::zero(), U256::from(3)),
            (H256::from_low_u64_be(2), U256::from(1)),
        ]);

        let code = "el buen fla";
        let account = Account {
            nonce: 155,
            balance: U256::from(414241124u32),
            bytecode_hash: Some(keccak256(code)),
        };

        insert_account(&mut *tx, address, account, &storage);
        tx.commit().unwrap();

        let got = StorageRoot::new(db.tx().unwrap(), address).root().unwrap();

        assert_eq!(storage_root(&storage), got);
    }
}
