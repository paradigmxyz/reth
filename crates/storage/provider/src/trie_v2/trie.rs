// 1. Calculate Root given entire presorted hashed thing
// Must work with:
// 1. Storage Trie Cursor
// 2. Accounts Trie Cursor
// 2. Update root given a list of updates
// Be able to calculate incremental state root without taking a write lock

use crate::trie_v2::hash_builder::HashBuilder;

use super::nibbles::Nibbles;

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
        let mut walker = cursor.walk_dup(Some(hashed_address), None)?;

        let mut hash_builder = HashBuilder::new();
        while let Some(item) = walker.next() {
            let (hashed_address, entry) = item?;
            let StorageEntry { key: hashed_slot, value } = entry;

            let nibbles = Nibbles::unpack(hashed_slot);
            hash_builder.add_leaf(nibbles, reth_rlp::encode_fixed_size(&value).as_ref());
        }

        let root = hash_builder.root();

        Ok(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Transaction;
    use proptest::{prelude::ProptestConfig, proptest};
    use reth_db::{
        database::Database, mdbx::test_utils::create_test_rw_db, tables, transaction::DbTxMut,
    };
    use reth_primitives::{keccak256, proofs::KeccakHasher, Address};
    use reth_rlp::encode_fixed_size;
    use std::collections::HashMap;

    use reth_primitives::{Account, H256, U256};

    fn insert_account<'a, TX: DbTxMut<'a>>(
        tx: &mut TX,
        address: Address,
        account: Account,
        storage: &HashMap<H256, U256>,
    ) {
        let hashed_address = keccak256(address);
        tx.put::<tables::HashedAccount>(hashed_address, account).unwrap();

        for (k, v) in storage {
            tx.put::<tables::HashedStorage>(
                hashed_address,
                StorageEntry { key: keccak256(k), value: *v },
            )
            .unwrap();
        }
    }

    fn storage_root<I: Iterator<Item = (H256, U256)>>(storage: I) -> H256 {
        let encoded_storage = storage.map(|(k, v)| (k, encode_fixed_size(&v).to_vec()));

        H256(triehash::sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage).0)
    }

    #[test]
    // TODO: Figure out why this fails.
    fn arbitrary_storage_root() {
        proptest!(ProptestConfig::with_cases(1), |(item: (Address, std::collections::BTreeMap<H256, U256>))| {
            let (address, storage) = item;

            let hashed_address = keccak256(address);
            let db = create_test_rw_db();
            let mut tx = Transaction::new(db.as_ref()).unwrap();
            for (key, value) in &storage {
                tx.put::<tables::HashedStorage>(
                    hashed_address,
                    StorageEntry { key: keccak256(key), value: *value },
                )
                .unwrap();
            }
            tx.commit().unwrap();

            let got = StorageRoot::new(db.tx().unwrap(), address).root().unwrap();
            let expected = storage_root(storage.into_iter());
            dbg!(&got, &expected);
            assert_eq!(expected, got);
        });
    }

    #[test]
    // This ensures that the walker goes over all the accounts.
    fn test_storage_root() {
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

        assert_eq!(storage_root(storage.into_iter()), got);
    }
}
