use crate::Transaction;
use cita_trie::{PatriciaTrie, Trie};
use hasher::HasherKeccak;
use reth_codecs::Compact;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::Database,
    models::{AccountBeforeTx, TransitionIdAddress},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_primitives::{
    keccak256, proofs::EMPTY_ROOT, Account, Address, StorageEntry, StorageTrieEntry, TransitionId,
    TrieStageProgress, H256, KECCAK_EMPTY, U256,
};
use reth_rlp::{
    encode_fixed_size, Decodable, DecodeError, Encodable, RlpDecodable, RlpEncodable,
    EMPTY_STRING_CODE,
};
use reth_tracing::tracing::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    sync::Arc,
};

/// Merkle Trie error types
#[allow(missing_docs)]
#[derive(Debug, thiserror::Error)]
pub enum TrieError {
    #[error("Some error occurred: {0}")]
    InternalError(#[from] cita_trie::TrieError),
    #[error("The root node wasn't found in the DB")]
    MissingRoot(H256),
    #[error("{0:?}")]
    DatabaseError(#[from] reth_db::Error),
    #[error("{0:?}")]
    DecodeError(#[from] DecodeError),
    #[error("Trie requires committing a checkpoint.")]
    UnexpectedCheckpoint,
}

/// Database wrapper implementing HashDB trait.
struct HashDatabase<'tx, 'itx, DB: Database> {
    tx: &'tx Transaction<'itx, DB>,
}

impl<'tx, 'itx, DB> cita_trie::DB for HashDatabase<'tx, 'itx, DB>
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

    fn insert(&self, _key: Vec<u8>, _value: Vec<u8>) -> Result<(), Self::Error> {
        unreachable!("Use batch instead.");
    }

    // Insert a batch of data into the cache.
    fn insert_batch(&self, keys: Vec<Vec<u8>>, values: Vec<Vec<u8>>) -> Result<(), Self::Error> {
        let mut cursor = self.tx.cursor_write::<tables::AccountsTrie>()?;
        for (key, value) in keys.into_iter().zip(values.into_iter()) {
            cursor.upsert(H256::from_slice(key.as_slice()), value)?;
        }
        Ok(())
    }

    fn remove_batch(&self, keys: &[Vec<u8>]) -> Result<(), Self::Error> {
        let mut cursor = self.tx.cursor_write::<tables::AccountsTrie>()?;
        for key in keys {
            if cursor.seek_exact(H256::from_slice(key.as_slice()))?.is_some() {
                cursor.delete_current()?;
            }
        }
        Ok(())
    }

    fn remove(&self, _key: &[u8]) -> Result<(), Self::Error> {
        unreachable!("Use batch instead.");
    }

    fn flush(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<'tx, 'itx, DB: Database> HashDatabase<'tx, 'itx, DB> {
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

/// Database wrapper implementing HashDB trait.
struct DupHashDatabase<'tx, 'itx, DB: Database> {
    tx: &'tx Transaction<'itx, DB>,
    key: H256,
}

impl<'tx, 'itx, DB> cita_trie::DB for DupHashDatabase<'tx, 'itx, DB>
where
    DB: Database,
{
    type Error = TrieError;

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Self::Error> {
        let mut cursor = self.tx.cursor_dup_read::<tables::StoragesTrie>()?;
        let subkey = H256::from_slice(key);
        Ok(cursor
            .seek_by_key_subkey(self.key, subkey)?
            .filter(|entry| entry.hash == subkey)
            .map(|entry| entry.node))
    }

    fn contains(&self, key: &[u8]) -> Result<bool, Self::Error> {
        Ok(<Self as cita_trie::DB>::get(self, key)?.is_some())
    }

    fn insert(&self, _key: Vec<u8>, _value: Vec<u8>) -> Result<(), Self::Error> {
        unreachable!("Use batch instead.");
    }

    /// Insert a batch of data into the cache.
    fn insert_batch(&self, keys: Vec<Vec<u8>>, values: Vec<Vec<u8>>) -> Result<(), Self::Error> {
        let mut cursor = self.tx.cursor_dup_write::<tables::StoragesTrie>()?;
        for (key, node) in keys.into_iter().zip(values.into_iter()) {
            let hash = H256::from_slice(key.as_slice());
            if cursor.seek_by_key_subkey(self.key, hash)?.filter(|e| e.hash == hash).is_some() {
                cursor.delete_current()?;
            }
            cursor.upsert(self.key, StorageTrieEntry { hash, node })?;
        }
        Ok(())
    }

    fn remove_batch(&self, keys: &[Vec<u8>]) -> Result<(), Self::Error> {
        let mut cursor = self.tx.cursor_dup_write::<tables::StoragesTrie>()?;
        for key in keys {
            let hash = H256::from_slice(key.as_slice());
            if cursor.seek_by_key_subkey(self.key, hash)?.filter(|e| e.hash == hash).is_some() {
                cursor.delete_current()?;
            }
        }
        Ok(())
    }

    fn remove(&self, _key: &[u8]) -> Result<(), Self::Error> {
        unreachable!("Use batch instead.");
    }

    fn flush(&self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl<'tx, 'itx, DB: Database> DupHashDatabase<'tx, 'itx, DB> {
    /// Instantiates a new Database for the storage trie, with an empty root
    fn new(tx: &'tx Transaction<'itx, DB>, key: H256) -> Result<Self, TrieError> {
        let root = EMPTY_ROOT;
        let mut cursor = tx.cursor_dup_write::<tables::StoragesTrie>()?;
        if cursor.seek_by_key_subkey(key, root)?.filter(|entry| entry.hash == root).is_none() {
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
            .filter(|entry| entry.hash == root)
            .ok_or(TrieError::MissingRoot(root))?;
        Ok(Self { tx, key })
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

/// Struct for calculating the root of a merkle patricia tree,
/// while populating the database with intermediate hashes.
#[derive(Debug)]
pub struct DBTrieLoader {
    /// The maximum number of keys to insert before committing. Both from `AccountsTrie` and
    /// `StoragesTrie`.
    pub commit_threshold: u64,
    /// The current number of inserted keys from both `AccountsTrie` and `StoragesTrie`.
    current: u64,
}

impl Default for DBTrieLoader {
    fn default() -> Self {
        DBTrieLoader { commit_threshold: 500_000, current: 0 }
    }
}

/// Status of the trie calculation.
#[derive(Debug, PartialEq, Copy, Clone)]
pub enum TrieProgress {
    /// Trie has finished with the passed root.
    Complete(H256),
    /// Trie has hit its commit threshold.
    InProgress(TrieStageProgress),
}

impl TrieProgress {
    /// Consumes the root from its `Complete` variant. If that's not possible, throw
    /// `TrieError::UnexpectedCheckpoint`.
    pub fn root(self) -> Result<H256, TrieError> {
        match self {
            Self::Complete(root) => Ok(root),
            _ => Err(TrieError::UnexpectedCheckpoint),
        }
    }
}

impl DBTrieLoader {
    /// Calculates the root of the state trie, saving intermediate hashes in the database.
    pub fn calculate_root<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
    ) -> Result<TrieProgress, TrieError> {
        let mut progress = self.get_progress(tx)?;

        if progress.next_hashed_account.is_none() {
            tx.clear::<tables::AccountsTrie>()?;
            tx.clear::<tables::StoragesTrie>()?;
        }

        let mut trie = PatriciaTrie::new(
            Arc::new(if let Some(root) = progress.current_account_root {
                HashDatabase::from_root(tx, root)?
            } else {
                HashDatabase::new(tx)?
            }),
            Arc::new(HasherKeccak::new()),
        );

        let mut accounts_cursor = tx.cursor_read::<tables::HashedAccount>()?;
        let mut walker = accounts_cursor.walk(progress.next_hashed_account.take())?;

        while let Some((hashed_address, account)) = walker.next().transpose()? {
            match self.calculate_storage_root(
                tx,
                hashed_address,
                progress.next_storage.take(),
                progress.current_storage_root.take(),
            )? {
                TrieProgress::Complete(root) => {
                    let value = EthAccount::from_with_root(account, root);

                    let mut out = Vec::new();
                    Encodable::encode(&value, &mut out);
                    trie.insert(hashed_address.as_bytes().to_vec(), out)?;

                    if self.has_hit_threshold() {
                        return self.save_account_progress(
                            Default::default(),
                            H256::from_slice(trie.root()?.as_slice()),
                            hashed_address,
                            tx,
                        )
                    }
                }
                TrieProgress::InProgress(in_progress) => {
                    return self.save_account_progress(
                        in_progress,
                        H256::from_slice(trie.root()?.as_slice()),
                        hashed_address,
                        tx,
                    )
                }
            }
        }

        // Reset inner stage progress
        self.save_progress(tx, Default::default())?;

        Ok(TrieProgress::Complete(H256::from_slice(trie.root()?.as_slice())))
    }

    fn calculate_storage_root<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
        address: H256,
        next_storage: Option<H256>,
        current_storage_root: Option<H256>,
    ) -> Result<TrieProgress, TrieError> {
        let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorage>()?;

        let (mut current_entry, db) = if let Some(entry) = next_storage {
            (
                storage_cursor.seek_by_key_subkey(address, entry)?.filter(|e| e.key == entry),
                DupHashDatabase::from_root(tx, address, current_storage_root.expect("is some"))?,
            )
        } else {
            (
                storage_cursor.seek_by_key_subkey(address, H256::zero())?,
                DupHashDatabase::new(tx, address)?,
            )
        };

        let mut trie = PatriciaTrie::new(Arc::new(db), Arc::new(HasherKeccak::new()));

        while let Some(StorageEntry { key: storage_key, value }) = current_entry {
            let out = encode_fixed_size(&value).to_vec();
            trie.insert(storage_key.to_vec(), out)?;
            // Should be able to use walk_dup, but any call to next() causes an assert fail in
            // mdbx.c
            current_entry = storage_cursor.next_dup()?.map(|(_, v)| v);
            let threshold = self.has_hit_threshold();
            if let Some(current_entry) = current_entry {
                if threshold {
                    return Ok(TrieProgress::InProgress(TrieStageProgress {
                        current_storage_root: Some(H256::from_slice(trie.root()?.as_slice())),
                        next_storage: Some(current_entry.key),
                        ..Default::default()
                    }))
                }
            }
        }

        Ok(TrieProgress::Complete(H256::from_slice(trie.root()?.as_slice())))
    }

    /// Calculates the root of the state trie by updating an existing trie.
    pub fn update_root<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
        mut root: H256,
        tid_range: Range<TransitionId>,
    ) -> Result<TrieProgress, TrieError> {
        let mut progress = self.get_progress(tx)?;

        if let Some(intermediate_root) = progress.current_account_root.take() {
            root = intermediate_root;
        }

        let next_acc = progress.next_hashed_account.take();
        let changed_accounts = self
            .gather_changes(tx, tid_range)?
            .into_iter()
            .skip_while(|(addr, _)| next_acc.is_some() && next_acc.expect("is some") != *addr);

        let mut trie = PatriciaTrie::from(
            Arc::new(HashDatabase::from_root(tx, root)?),
            Arc::new(HasherKeccak::new()),
            root.as_bytes(),
        )?;

        let mut accounts_cursor = tx.cursor_read::<tables::HashedAccount>()?;

        for (hashed_address, changed_storages) in changed_accounts {
            let res = if let Some(account) = trie.get(hashed_address.as_slice())? {
                trie.remove(hashed_address.as_bytes())?;

                let storage_root = EthAccount::decode(&mut account.as_slice())?.storage_root;
                self.update_storage_root(
                    tx,
                    progress.current_storage_root.take().unwrap_or(storage_root),
                    hashed_address,
                    changed_storages,
                    progress.next_storage.take(),
                )?
            } else {
                self.calculate_storage_root(
                    tx,
                    hashed_address,
                    progress.next_storage.take(),
                    progress.current_storage_root.take(),
                )?
            };

            let storage_root = match res {
                TrieProgress::Complete(root) => root,
                TrieProgress::InProgress(in_progress) => {
                    return self.save_account_progress(
                        in_progress,
                        H256::from_slice(trie.root()?.as_slice()),
                        hashed_address,
                        tx,
                    )
                }
            };

            if let Some((_, account)) = accounts_cursor.seek_exact(hashed_address)? {
                let value = EthAccount::from_with_root(account, storage_root);

                let mut out = Vec::new();
                Encodable::encode(&value, &mut out);

                trie.insert(hashed_address.as_bytes().to_vec(), out)?;

                if self.has_hit_threshold() {
                    return self.save_account_progress(
                        Default::default(),
                        H256::from_slice(trie.root()?.as_slice()),
                        hashed_address,
                        tx,
                    )
                }
            }
        }
        let root = H256::from_slice(trie.root()?.as_slice());

        Ok(TrieProgress::Complete(root))
    }

    fn update_storage_root<DB: Database>(
        &mut self,
        tx: &Transaction<'_, DB>,
        root: H256,
        address: H256,
        changed_storages: BTreeSet<H256>,
        next_storage: Option<H256>,
    ) -> Result<TrieProgress, TrieError> {
        let mut storage_cursor = tx.cursor_dup_read::<tables::HashedStorage>()?;

        let mut trie = PatriciaTrie::new(
            Arc::new(DupHashDatabase::from_root(tx, address, root)?),
            Arc::new(HasherKeccak::new()),
        );

        let changed_storages = changed_storages
            .into_iter()
            .skip_while(|k| next_storage.is_some() && *k == next_storage.expect("is some"));

        for key in changed_storages {
            if let Some(StorageEntry { value, .. }) =
                storage_cursor.seek_by_key_subkey(address, key)?.filter(|e| e.key == key)
            {
                let out = encode_fixed_size(&value).to_vec();
                trie.insert(key.as_bytes().to_vec(), out)?;
                if self.has_hit_threshold() {
                    return Ok(TrieProgress::InProgress(TrieStageProgress {
                        current_storage_root: Some(H256::from_slice(trie.root()?.as_slice())),
                        next_storage: Some(key),
                        ..Default::default()
                    }))
                }
            } else {
                trie.remove(key.as_bytes())?;
            }
        }

        let root = H256::from_slice(trie.root()?.as_slice());

        Ok(TrieProgress::Complete(root))
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

    fn save_account_progress<DB: Database>(
        &mut self,
        mut in_progress: TrieStageProgress,
        root: H256,
        hashed_address: H256,
        tx: &Transaction<'_, DB>,
    ) -> Result<TrieProgress, TrieError> {
        in_progress.current_account_root = Some(root);
        in_progress.next_hashed_account = Some(hashed_address);

        self.save_progress(tx, in_progress)?;

        Ok(TrieProgress::InProgress(in_progress))
    }

    /// Saves the trie progress
    pub fn save_progress<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
        progress: TrieStageProgress,
    ) -> Result<(), TrieError> {
        let mut buf = vec![];
        progress.to_compact(&mut buf);
        Ok(tx.put::<tables::SyncStageProgress>("TrieLoader".into(), buf)?)
    }

    /// Gets the trie progress
    pub fn get_progress<DB: Database>(
        &self,
        tx: &Transaction<'_, DB>,
    ) -> Result<TrieStageProgress, TrieError> {
        let buf = tx.get::<tables::SyncStageProgress>("TrieLoader".into())?.unwrap_or_default();
        let (progress, _) = TrieStageProgress::from_compact(&buf, buf.len());
        Ok(progress)
    }

    fn has_hit_threshold(&mut self) -> bool {
        self.current += 1;
        self.current >= self.commit_threshold
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
        Address, ChainSpec, MAINNET,
    };
    use std::{collections::HashMap, str::FromStr};
    use triehash::sec_trie_root;

    #[test]
    fn empty_trie() {
        let mut trie = DBTrieLoader::default();
        let db = create_test_rw_db();
        let tx = Transaction::new(db.as_ref()).unwrap();
        assert_matches!(trie.calculate_root(&tx), Ok(got) if got.root().unwrap() == EMPTY_ROOT);
    }

    #[test]
    fn single_account_trie() {
        let mut trie = DBTrieLoader::default();
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
            Ok(got) if got.root().unwrap() == expected
        );
    }

    #[test]
    fn two_accounts_trie() {
        let mut trie = DBTrieLoader::default();
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
            Ok(got) if got.root().unwrap() == expected
        );
    }

    #[test]
    fn single_storage_trie() {
        let mut trie = DBTrieLoader::default();
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
            trie.calculate_storage_root(&tx, hashed_address, None, None),
            Ok(got) if got.root().unwrap() == expected
        );
    }

    #[test]
    fn single_account_with_storage_trie() {
        let mut trie = DBTrieLoader::default();
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
            Ok(got) if got.root().unwrap() == expected
        );
    }

    #[test]
    fn verify_genesis() {
        let mut trie = DBTrieLoader::default();
        let db = create_test_rw_db();
        let mut tx = Transaction::new(db.as_ref()).unwrap();
        let ChainSpec { genesis, .. } = MAINNET.clone();

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
            Ok(got) if got.root().unwrap() == state_root
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
        let mut trie = DBTrieLoader::default();
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
            Ok(got) if got.root().unwrap() == expected
        , "where expected is {expected:?}");
    }

    #[test]
    fn arbitrary() {
        proptest!(ProptestConfig::with_cases(10), |(accounts: BTreeMap<Address, (Account, BTreeSet<StorageEntry>)>)| {
            test_with_accounts(accounts);
        });
    }
}
