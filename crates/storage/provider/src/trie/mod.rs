use crate::Transaction;
use cita_trie::{PatriciaTrie, Trie};
use hasher::HasherKeccak;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    models::{AccountBeforeTx, TransitionIdAddress},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{
    keccak256, proofs::EMPTY_ROOT, Account, Address, StorageEntry, StorageTrieEntry, TransitionId,
    H256, KECCAK_EMPTY, U256,
};
use reth_rlp::{
    encode_fixed_size, Decodable, DecodeError, Encodable, RlpDecodable, RlpEncodable,
    EMPTY_STRING_CODE,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    marker::PhantomData,
    ops::Range,
    sync::Arc,
};

/// Errors returned by [`DBTrieLoader`].
#[derive(Debug, thiserror::Error)]
pub enum TrieError {
    /// Error returned by the underlying implementation.
    #[error("Some error occurred: {0}")]
    InternalError(#[from] cita_trie::TrieError),
    /// The database doesn't contain the root of the trie.
    #[error("The root node wasn't found in the DB")]
    MissingRoot(H256),
    /// Error returned by the database.
    #[error("{0:?}")]
    DatabaseError(#[from] reth_db::Error),
    /// Error when encoding/decoding a value.
    #[error("{0:?}")]
    DecodeError(#[from] DecodeError),
}

/// Database wrapper implementing HashDB trait, with a read-write transaction.
struct HashDatabaseMut<'tx, 'itx, DB: Database> {
    tx: &'tx Transaction<'itx, DB>,
}

impl<'tx, 'itx, DB> cita_trie::DB for HashDatabaseMut<'tx, 'itx, DB>
where
    DB: Database,
{
    type Error = TrieError;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(self.tx.get::<tables::AccountsTrie>(H256::from_slice(key))?)
    }

    fn contains(&self, key: &[u8]) -> Result<bool, Self::Error> {
        Ok(<Self as cita_trie::DB>::get(self, key)?.is_some())
    }

    fn insert(&self, key: Vec<u8>, value: Vec<u8>) -> Result<(), Self::Error> {
        // Caching and bulk inserting shouldn't be needed, as the data is ordered
        self.tx.put::<tables::AccountsTrie>(H256::from_slice(key.as_slice()), value)?;
        Ok(())
    }

    fn remove(&self, key: &[u8]) -> Result<(), Self::Error> {
        self.tx.delete::<tables::AccountsTrie>(H256::from_slice(key), None)?;
        Ok(())
    }

    fn flush(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<'tx, 'itx, DB: Database> HashDatabaseMut<'tx, 'itx, DB> {
    /// Instantiates a new Database for the accounts trie, with an empty root
    fn new(tx: &'tx Transaction<'itx, DB>) -> Result<Self, TrieError> {
        let root = EMPTY_ROOT;
        if tx.get::<tables::AccountsTrie>(root)?.is_none() {
            tx.put::<tables::AccountsTrie>(root, [EMPTY_STRING_CODE].to_vec())?;
        }
        Ok(Self { tx })
    }

    /// Instantiates a new Database for the accounts trie, with an existing root
    fn from_root(tx: &'tx Transaction<'itx, DB>, root: H256) -> Result<Self, TrieError> {
        if root == EMPTY_ROOT {
            return Self::new(tx)
        }
        tx.get::<tables::AccountsTrie>(root)?.ok_or(TrieError::MissingRoot(root))?;
        Ok(Self { tx })
    }
}

/// Database wrapper implementing HashDB trait, with a read-write transaction.
struct DupHashDatabaseMut<'tx, 'itx, DB: Database> {
    tx: &'tx Transaction<'itx, DB>,
    key: H256,
}

impl<'tx, 'itx, DB> cita_trie::DB for DupHashDatabaseMut<'tx, 'itx, DB>
where
    DB: Database,
{
    type Error = TrieError;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        let mut cursor = self.tx.cursor_dup_read::<tables::StoragesTrie>()?;
        Ok(cursor.seek_by_key_subkey(self.key, H256::from_slice(key))?.map(|entry| entry.node))
    }

    fn contains(&self, key: &[u8]) -> Result<bool, Self::Error> {
        Ok(<Self as cita_trie::DB>::get(self, key)?.is_some())
    }

    fn insert(&self, key: Vec<u8>, value: Vec<u8>) -> Result<(), Self::Error> {
        // Caching and bulk inserting shouldn't be needed, as the data is ordered
        self.tx.put::<tables::StoragesTrie>(
            self.key,
            StorageTrieEntry { hash: H256::from_slice(key.as_slice()), node: value },
        )?;
        Ok(())
    }

    fn remove(&self, key: &[u8]) -> Result<(), Self::Error> {
        let mut cursor = self.tx.cursor_dup_write::<tables::StoragesTrie>()?;
        cursor
            .seek_by_key_subkey(self.key, H256::from_slice(key))?
            .map(|_| cursor.delete_current())
            .transpose()?;
        Ok(())
    }

    fn flush(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<'tx, 'itx, DB: Database> DupHashDatabaseMut<'tx, 'itx, DB> {
    /// Instantiates a new Database for the storage trie, with an empty root
    fn new(tx: &'tx Transaction<'itx, DB>, key: H256) -> Result<Self, TrieError> {
        let root = EMPTY_ROOT;
        let mut cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;
        if cursor.seek_by_key_subkey(key, root)?.is_none() {
            tx.put::<tables::StoragesTrie>(
                key,
                StorageTrieEntry { hash: root, node: [EMPTY_STRING_CODE].to_vec() },
            )?;
        }
        Ok(Self { tx, key })
    }

    /// Instantiates a new Database for the storage trie, with an existing root
    fn from_root(tx: &'tx Transaction<'itx, DB>, key: H256, root: H256) -> Result<Self, TrieError> {
        if root == EMPTY_ROOT {
            return Self::new(tx, key)
        }
        tx.cursor_dup_read::<tables::StoragesTrie>()?
            .seek_by_key_subkey(key, root)?
            .ok_or(TrieError::MissingRoot(root))?;
        Ok(Self { tx, key })
    }
}

/// Database wrapper implementing HashDB trait, with a read-only transaction.
struct HashDatabase<'tx, 'itx, TX: DbTx<'itx>> {
    tx: &'tx TX,
    _p: PhantomData<&'itx ()>, // to suppress "unused" lifetime 'itx
}

impl<'tx, 'itx, TX> cita_trie::DB for HashDatabase<'tx, 'itx, TX>
where
    TX: DbTx<'itx>,
{
    type Error = TrieError;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        Ok(self.tx.get::<tables::AccountsTrie>(H256::from_slice(key))?)
    }

    fn contains(&self, key: &[u8]) -> Result<bool, Self::Error> {
        Ok(<Self as cita_trie::DB>::get(self, key)?.is_some())
    }

    fn insert(&self, _key: Vec<u8>, _value: Vec<u8>) -> Result<(), Self::Error> {
        // this could be avoided if cita_trie::DB was split into two traits
        // with read and write operations respectively
        unimplemented!("insert isn't valid for read-only transaction");
    }

    fn remove(&self, _key: &[u8]) -> Result<(), Self::Error> {
        unimplemented!("remove isn't valid for read-only transaction");
    }

    fn flush(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<'tx, 'itx, TX: DbTx<'itx>> HashDatabase<'tx, 'itx, TX> {
    /// Instantiates a new Database for the accounts trie, with an existing root
    fn from_root(tx: &'tx TX, root: H256) -> Result<Self, TrieError> {
        tx.get::<tables::AccountsTrie>(root)?.ok_or(TrieError::MissingRoot(root))?;
        Ok(Self { tx, _p: Default::default() })
    }
}

/// Database wrapper implementing HashDB trait, with a read-only transaction.
struct DupHashDatabase<'tx, 'itx, TX: DbTx<'itx>> {
    tx: &'tx TX,
    key: H256,
    _p: PhantomData<&'itx ()>, // to suppress "unused" lifetime 'itx
}

impl<'tx, 'itx, TX> cita_trie::DB for DupHashDatabase<'tx, 'itx, TX>
where
    TX: DbTx<'itx>,
{
    type Error = TrieError;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        let mut cursor = self.tx.cursor_dup_read::<tables::StoragesTrie>()?;
        Ok(cursor.seek_by_key_subkey(self.key, H256::from_slice(key))?.map(|entry| entry.node))
    }

    fn contains(&self, key: &[u8]) -> Result<bool, Self::Error> {
        Ok(<Self as cita_trie::DB>::get(self, key)?.is_some())
    }

    fn insert(&self, _key: Vec<u8>, _value: Vec<u8>) -> Result<(), Self::Error> {
        // Caching and bulk inserting shouldn't be needed, as the data is ordered
        unimplemented!("insert isn't valid for read-only transaction");
    }

    fn remove(&self, _key: &[u8]) -> Result<(), Self::Error> {
        unimplemented!("remove isn't valid for read-only transaction");
    }

    fn flush(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<'tx, 'itx, TX: DbTx<'itx>> DupHashDatabase<'tx, 'itx, TX> {
    /// Instantiates a new Database for the storage trie, with an existing root
    fn from_root(tx: &'tx TX, key: H256, root: H256) -> Result<Self, TrieError> {
        tx.cursor_dup_read::<tables::StoragesTrie>()?
            .seek_by_key_subkey(key, root)?
            .ok_or(TrieError::MissingRoot(root))?;
        Ok(Self { tx, key, _p: Default::default() })
    }
}

/// An Ethereum account, for RLP encoding traits deriving.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
pub(crate) struct EthAccount {
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
            storage_root: EMPTY_ROOT,
            code_hash: acc.bytecode_hash.unwrap_or(KECCAK_EMPTY),
        }
    }
}

impl EthAccount {
    pub(crate) fn from_with_root(acc: Account, storage_root: H256) -> EthAccount {
        Self { storage_root, ..Self::from(acc) }
    }
}

/// Helper struct that interacts with a Merkle Patricia Tree persisted
/// in the database.
#[derive(Debug, Default)]
pub struct DBTrieLoader;

impl DBTrieLoader {
    /// Calculates the root of the state trie, saving intermediate hashes in the database.
    pub fn calculate_root<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
    ) -> Result<H256, TrieError> {
        tx.clear::<tables::AccountsTrie>()?;
        tx.clear::<tables::StoragesTrie>()?;

        let mut accounts_cursor = tx.cursor_read::<tables::HashedAccount>()?;
        let mut walker = accounts_cursor.walk(None)?;

        let db = Arc::new(HashDatabaseMut::new(tx)?);

        let hasher = Arc::new(HasherKeccak::new());

        let mut trie = PatriciaTrie::new(Arc::clone(&db), Arc::clone(&hasher));

        while let Some((hashed_address, account)) = walker.next().transpose()? {
            let value = EthAccount::from_with_root(
                account,
                self.calculate_storage_root(tx, hashed_address)?,
            );

            let mut out = Vec::new();
            Encodable::encode(&value, &mut out);
            trie.insert(hashed_address.as_bytes().to_vec(), out)?;
        }
        let root = H256::from_slice(trie.root()?.as_slice());

        Ok(root)
    }

    fn calculate_storage_root<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        address: H256,
    ) -> Result<H256, TrieError> {
        let db = Arc::new(DupHashDatabaseMut::new(tx, address)?);

        let hasher = Arc::new(HasherKeccak::new());

        let mut trie = PatriciaTrie::new(Arc::clone(&db), Arc::clone(&hasher));

        let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorage>()?;

        // Should be able to use walk_dup, but any call to next() causes an assert fail in mdbx.c
        // let mut walker = storage_cursor.walk_dup(address, H256::zero())?;
        let mut current = storage_cursor.seek_by_key_subkey(address, H256::zero())?;

        while let Some(StorageEntry { key: storage_key, value }) = current {
            let out = encode_fixed_size(&value).to_vec();
            trie.insert(storage_key.to_vec(), out)?;
            current = storage_cursor.next_dup()?.map(|(_, v)| v);
        }

        let root = H256::from_slice(trie.root()?.as_slice());

        Ok(root)
    }

    /// Calculates the root of the state trie by updating an existing trie.
    pub fn update_root<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        root: H256,
        tid_range: Range<TransitionId>,
    ) -> Result<H256, TrieError> {
        let mut accounts_cursor = tx.cursor_read::<tables::HashedAccount>()?;

        let changed_accounts = self.gather_changes(tx, tid_range)?;

        let db = Arc::new(HashDatabaseMut::from_root(tx, root)?);

        let hasher = Arc::new(HasherKeccak::new());

        let mut trie = PatriciaTrie::from(Arc::clone(&db), Arc::clone(&hasher), root.as_bytes())?;

        for (address, changed_storages) in changed_accounts {
            if let Some(account) = trie.get(address.as_slice())? {
                let storage_root = EthAccount::decode(&mut account.as_slice())?.storage_root;
                trie.remove(address.as_bytes())?;

                if let Some((_, account)) = accounts_cursor.seek_exact(address)? {
                    let value = EthAccount::from_with_root(
                        account,
                        self.update_storage_root(tx, storage_root, address, changed_storages)?,
                    );

                    let mut out = Vec::new();
                    Encodable::encode(&value, &mut out);
                    trie.insert(address.as_bytes().to_vec(), out)?;
                }
            }
        }

        let root = H256::from_slice(trie.root()?.as_slice());

        Ok(root)
    }

    fn update_storage_root<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        root: H256,
        address: H256,
        changed_storages: BTreeSet<H256>,
    ) -> Result<H256, TrieError> {
        let db = Arc::new(DupHashDatabaseMut::from_root(tx, address, root)?);

        let hasher = Arc::new(HasherKeccak::new());

        let mut trie = PatriciaTrie::from(Arc::clone(&db), Arc::clone(&hasher), root.as_bytes())?;
        let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorage>()?;

        for key in changed_storages {
            if let Some(StorageEntry { value, .. }) =
                storage_cursor.seek_by_key_subkey(address, key)?
            {
                let out = encode_fixed_size(&value).to_vec();
                trie.insert(key.as_bytes().to_vec(), out)?;
            } else {
                trie.remove(key.as_bytes())?;
            }
        }

        let root = H256::from_slice(trie.root()?.as_slice());

        Ok(root)
    }

    fn gather_changes<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        tid_range: Range<TransitionId>,
    ) -> Result<BTreeMap<H256, BTreeSet<H256>>, TrieError> {
        let mut account_cursor = tx.cursor_read::<tables::AccountChangeSet>()?;

        let mut account_changes: BTreeMap<Address, BTreeSet<H256>> = BTreeMap::new();

        let mut walker = account_cursor.walk_range(tid_range.clone())?;

        while let Some((_, AccountBeforeTx { address, .. })) = walker.next().transpose()? {
            account_changes.insert(address, Default::default());
        }

        let mut storage_cursor = tx.cursor_dup_read::<tables::StorageChangeSet>()?;

        let start = TransitionIdAddress((tid_range.start, Address::zero()));
        let end = TransitionIdAddress((tid_range.end, Address::zero()));
        let mut walker = storage_cursor.walk_range(start..end)?;

        while let Some((TransitionIdAddress((_, address)), StorageEntry { key, .. })) =
            walker.next().transpose()?
        {
            account_changes.entry(address).or_default().insert(key);
        }

        let hashed_changes = account_changes
            .into_iter()
            .map(|(address, storage)| {
                (keccak256(address), storage.into_iter().map(keccak256).collect())
            })
            .collect();

        Ok(hashed_changes)
    }

    /// Returns a list of encoded nodes from the root of the trie to the given
    /// account, the account's storage root hash, and optionally a list of
    /// proofs for the given keys (starting at the storage root).
    pub fn generate_acount_proof<'tx, 'itx>(
        &self,
        tx: &'tx impl DbTx<'itx>,
        root: H256,
        address: H256,
        keys: Vec<H256>,
    ) -> Result<(Vec<Vec<u8>>, H256, Vec<Vec<Vec<u8>>>), TrieError> {
        let db = Arc::new(HashDatabase::from_root(tx, root)?);
        let hasher = Arc::new(HasherKeccak::new());

        let trie = PatriciaTrie::from(Arc::clone(&db), Arc::clone(&hasher), root.as_bytes())?;
        let proof = trie.get_proof(keccak256(address).as_bytes())?;

        let Some(account) = trie.get(address.as_slice())? else { return Ok((proof, KECCAK_EMPTY, vec![])) };

        let storage_root = EthAccount::decode(&mut account.as_slice())?.storage_root;

        Ok((proof, storage_root, self.generate_storage_proofs(tx, storage_root, address, keys)?))
    }

    fn generate_storage_proofs<'tx, 'itx>(
        &self,
        tx: &'tx impl DbTx<'itx>,
        root: H256,
        address: H256,
        keys: Vec<H256>,
    ) -> Result<Vec<Vec<Vec<u8>>>, TrieError> {
        let db = Arc::new(DupHashDatabase::from_root(tx, address, root)?);
        let hasher = Arc::new(HasherKeccak::new());

        let trie = PatriciaTrie::from(Arc::clone(&db), Arc::clone(&hasher), root.as_bytes())?;

        let proof = keys
            .into_iter()
            .map(|key| trie.get_proof(key.as_bytes()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(proof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use proptest::{prelude::ProptestConfig, proptest};
    use reth_db::{mdbx::test_utils::create_test_rw_db, tables, transaction::DbTxMut};
    use reth_primitives::{
        hex_literal::hex,
        keccak256,
        proofs::{genesis_state_root, KeccakHasher, EMPTY_ROOT},
        Address, ChainSpec,
    };
    use reth_staged_sync::utils::chainspec::chain_spec_value_parser;
    use std::{collections::HashMap, str::FromStr};
    use triehash::sec_trie_root;

    #[test]
    fn empty_trie() {
        let trie = DBTrieLoader::default();
        let db = create_test_rw_db();
        let tx = Transaction::new(db.as_ref()).unwrap();
        assert_matches!(trie.calculate_root(&tx), Ok(got) if got == EMPTY_ROOT);
    }

    #[test]
    fn single_account_trie() {
        let trie = DBTrieLoader::default();
        let db = create_test_rw_db();
        let tx = Transaction::new(db.as_ref()).unwrap();
        let address = Address::from_str("9fe4abd71ad081f091bd06dd1c16f7e92927561e").unwrap();
        let account = Account { nonce: 0, balance: U256::ZERO, bytecode_hash: None };
        tx.put::<tables::HashedAccount>(keccak256(address), account).unwrap();
        let mut encoded_account = Vec::new();
        EthAccount::from(account).encode(&mut encoded_account);
        let expected = H256(sec_trie_root::<KeccakHasher, _, _, _>([(address, encoded_account)]).0);
        assert_matches!(
            trie.calculate_root(&tx),
            Ok(got) if got == expected
        );
    }

    #[test]
    fn two_accounts_trie() {
        let trie = DBTrieLoader::default();
        let db = create_test_rw_db();
        let tx = Transaction::new(db.as_ref()).unwrap();

        let accounts = [
            (
                Address::from(hex!("9fe4abd71ad081f091bd06dd1c16f7e92927561e")),
                Account { nonce: 155, balance: U256::from(414241124), bytecode_hash: None },
            ),
            (
                Address::from(hex!("f8a6edaad4a332e6e550d0915a7fd5300b0b12d1")),
                Account { nonce: 3, balance: U256::from(78978), bytecode_hash: None },
            ),
        ];
        for (address, account) in accounts {
            tx.put::<tables::HashedAccount>(keccak256(address), account).unwrap();
        }
        let encoded_accounts = accounts.iter().map(|(k, v)| {
            let mut out = Vec::new();
            EthAccount::from(*v).encode(&mut out);
            (k, out)
        });
        let expected = H256(sec_trie_root::<KeccakHasher, _, _, _>(encoded_accounts).0);
        assert_matches!(
            trie.calculate_root(&tx),
            Ok(got) if got == expected
        );
    }

    #[test]
    fn single_storage_trie() {
        let trie = DBTrieLoader::default();
        let db = create_test_rw_db();
        let tx = Transaction::new(db.as_ref()).unwrap();

        let address = Address::from_str("9fe4abd71ad081f091bd06dd1c16f7e92927561e").unwrap();
        let hashed_address = keccak256(address);

        let storage = Vec::from([(H256::from_low_u64_be(2), U256::from(1))]);
        for (k, v) in storage.clone() {
            tx.put::<tables::HashedStorage>(
                hashed_address,
                StorageEntry { key: keccak256(k), value: v },
            )
            .unwrap();
        }
        let encoded_storage = storage.iter().map(|(k, v)| {
            let out = encode_fixed_size(v).to_vec();
            (k, out)
        });
        let expected = H256(sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage).0);
        assert_matches!(
            trie.calculate_storage_root(&tx, hashed_address),
            Ok(got) if got == expected
        );
    }

    #[test]
    fn single_account_with_storage_trie() {
        let trie = DBTrieLoader::default();
        let db = create_test_rw_db();
        let tx = Transaction::new(db.as_ref()).unwrap();

        let address = Address::from_str("9fe4abd71ad081f091bd06dd1c16f7e92927561e").unwrap();
        let hashed_address = keccak256(address);

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
        tx.put::<tables::HashedAccount>(hashed_address, account).unwrap();

        for (k, v) in storage.clone() {
            tx.put::<tables::HashedStorage>(
                hashed_address,
                StorageEntry { key: keccak256(k), value: v },
            )
            .unwrap();
        }
        let mut out = Vec::new();

        let encoded_storage = storage.iter().map(|(k, v)| {
            let out = encode_fixed_size(v).to_vec();
            (k, out)
        });

        let eth_account = EthAccount::from_with_root(
            account,
            H256(sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage).0),
        );
        eth_account.encode(&mut out);

        let expected = H256(sec_trie_root::<KeccakHasher, _, _, _>([(address, out)]).0);
        assert_matches!(
            trie.calculate_root(&tx),
            Ok(got) if got == expected
        );
    }

    #[test]
    fn verify_genesis() {
        let trie = DBTrieLoader::default();
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();
        let ChainSpec { genesis, .. } = chain_spec_value_parser("mainnet").unwrap();

        // Insert account state
        for (address, account) in &genesis.alloc {
            tx.put::<tables::HashedAccount>(
                keccak256(address),
                Account {
                    nonce: account.nonce.unwrap_or_default(),
                    balance: account.balance,
                    bytecode_hash: None,
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let state_root = genesis_state_root(&genesis.alloc);

        assert_matches!(
            trie.calculate_root(&tx),
            Ok(got) if got == state_root
        );
    }

    #[test]
    fn gather_changes() {
        let db = create_test_rw_db();
        let tx = Transaction::new(db.as_ref()).unwrap();

        let address = Address::from_str("9fe4abd71ad081f091bd06dd1c16f7e92927561e").unwrap();
        let hashed_address = keccak256(address);

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
        tx.put::<tables::HashedAccount>(hashed_address, account).unwrap();
        tx.put::<tables::AccountChangeSet>(31, AccountBeforeTx { address, info: None }).unwrap();

        for (k, v) in storage {
            tx.put::<tables::HashedStorage>(
                hashed_address,
                StorageEntry { key: keccak256(k), value: v },
            )
            .unwrap();
            tx.put::<tables::StorageChangeSet>(
                (32, address).into(),
                StorageEntry { key: k, value: U256::ZERO },
            )
            .unwrap();
        }

        let expected = BTreeMap::from([(
            hashed_address,
            BTreeSet::from([keccak256(H256::zero()), keccak256(H256::from_low_u64_be(2))]),
        )]);
        assert_matches!(
            DBTrieLoader::default().gather_changes(&tx, 32..33),
            Ok(got) if got == expected
        );
    }

    fn test_with_accounts(accounts: BTreeMap<Address, (Account, BTreeSet<StorageEntry>)>) {
        let trie = DBTrieLoader::default();
        let db = create_test_rw_db();
        let tx = Transaction::new(db.as_ref()).unwrap();

        let encoded_accounts = accounts
            .into_iter()
            .map(|(address, (account, storage))| {
                let hashed_address = keccak256(address);
                tx.put::<tables::HashedAccount>(hashed_address, account).unwrap();
                // This is to mimic real data. Only contract accounts have storage.
                let storage_root = if account.has_bytecode() {
                    let encoded_storage = storage.into_iter().map(|StorageEntry { key, value }| {
                        let hashed_key = keccak256(key);
                        let out = encode_fixed_size(&value).to_vec();
                        tx.put::<tables::HashedStorage>(
                            hashed_address,
                            StorageEntry { key: hashed_key, value },
                        )
                        .unwrap();
                        (key, out)
                    });
                    H256(sec_trie_root::<KeccakHasher, _, _, _>(encoded_storage).0)
                } else {
                    EMPTY_ROOT
                };
                let mut out = Vec::new();
                EthAccount::from_with_root(account, storage_root).encode(&mut out);
                (address, out)
            })
            .collect::<Vec<(Address, Vec<u8>)>>();

        let expected = H256(sec_trie_root::<KeccakHasher, _, _, _>(encoded_accounts).0);
        assert_matches!(
            trie.calculate_root(&tx),
            Ok(got) if got == expected
        , "where expected is {expected:?}");
    }

    #[test]
    fn arbitrary() {
        proptest!(ProptestConfig::with_cases(10), |(accounts: BTreeMap<Address, (Account, BTreeSet<StorageEntry>)>)| {
            test_with_accounts(accounts);
        });
    }
}
